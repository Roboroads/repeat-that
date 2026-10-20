//! The worker thread: drains captured audio from the capture's queue into the [`Recorder`].

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rt_capture::CaptureFormat;
use rt_core::{AudioFormat, EnergyVad, Recorder, SegmenterConfig, SileroVad, Vad};

use crate::status::StatusHub;
use crate::{EngineConfig, Sensitivity};

/// How often the worker drains the queue.
const DRAIN_EVERY: Duration = Duration::from_millis(10);
/// The recorder takes captured audio in parts of at most this many milliseconds, with the lock released in between.
const PART_MS: usize = 50;
/// How often `buffered` is updated in the status while it changes.
const STATUS_EVERY: Duration = Duration::from_millis(500);

/// The recorder, shared with the player thread, which renders from its buffer.
#[derive(Default)]
pub(crate) struct Core {
    /// `None` until the first audio comes in: the recorder needs its format.
    pub(crate) recorder: Option<Recorder>,
    /// The format the recorder takes.
    pub(crate) format: Option<AudioFormat>,
    /// Increases every time the recorder is replaced, so the player knows its utterance ids are gone.
    pub(crate) generation: u64,
    /// Whether captured audio goes into the recorder. Off from the moment a wipe starts until the next capture is
    /// attached, so audio still on its way from the stopping capture is dropped.
    pub(crate) accepting: bool,
}

pub(crate) type SharedCore = Arc<Mutex<Core>>;

pub(crate) fn lock(core: &SharedCore) -> std::sync::MutexGuard<'_, Core> {
    core.lock().unwrap_or_else(|e| e.into_inner())
}

/// The worker's end of a capture's queues: interleaved samples, and where in them the format changes.
pub(crate) struct CaptureQueue {
    pub(crate) samples: rtrb::Consumer<f32>,
    pub(crate) formats: rtrb::Consumer<FormatChange>,
    /// The format of the samples being taken; `None` before the first ones.
    pub(crate) format: Option<AudioFormat>,
    /// Samples taken from the queue so far.
    pub(crate) taken: u64,
}

impl CaptureQueue {
    pub(crate) fn new(samples: rtrb::Consumer<f32>, formats: rtrb::Consumer<FormatChange>) -> Self {
        Self {
            samples,
            formats,
            format: None,
            taken: 0,
        }
    }
}

/// From sample `at` of a capture's queue on, the samples are in `format`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FormatChange {
    pub(crate) at: u64,
    pub(crate) format: CaptureFormat,
}

pub(crate) enum WorkerCmd {
    /// Audio of a new capture comes in through this queue.
    Attach(CaptureQueue),
    /// The capture ended by itself: take what's left, close the sentence in progress, keep the buffer.
    Detach,
    /// Capture stopped for a wipe: drop the queue and everything buffered.
    Clear,
    SetConfig(EngineConfig),
    Shutdown,
}

/// Where the VAD comes from. It's created once, on the worker thread, when the first audio comes in.
pub(crate) enum VadSource {
    Silero,
    Given(Box<dyn Vad>),
}

/// One VAD shared by the recorders the worker creates over time (there's one at a time).
#[derive(Clone)]
struct SharedVad(Arc<Mutex<Box<dyn Vad>>>);

impl Vad for SharedVad {
    fn frame_len(&self) -> usize {
        self.lock().frame_len()
    }

    fn speech_probability(&mut self, frame: &[f32]) -> f32 {
        self.lock().speech_probability(frame)
    }

    fn reset(&mut self) {
        self.lock().reset();
    }
}

impl SharedVad {
    fn lock(&self) -> std::sync::MutexGuard<'_, Box<dyn Vad>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub(crate) struct Worker {
    core: SharedCore,
    status: Arc<StatusHub>,
    buffer: Duration,
    sensitivity: Sensitivity,
    vad_source: Option<VadSource>,
    vad: Option<SharedVad>,
    queue: Option<CaptureQueue>,
    /// Drained samples on their way into the recorder.
    scratch: Vec<f32>,
    last_status: Instant,
}

impl Worker {
    pub(crate) fn new(core: SharedCore, status: Arc<StatusHub>, config: &EngineConfig, vad: VadSource) -> Self {
        Self {
            core,
            status,
            buffer: config.buffer,
            sensitivity: config.sensitivity,
            vad_source: Some(vad),
            vad: None,
            queue: None,
            scratch: Vec::new(),
            last_status: Instant::now(),
        }
    }

    pub(crate) fn run(mut self, commands: Receiver<WorkerCmd>) {
        loop {
            match commands.recv_timeout(DRAIN_EVERY) {
                Ok(WorkerCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.handle(command),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.drain();
            if self.last_status.elapsed() >= STATUS_EVERY {
                self.report();
            }
        }
        self.clear();
    }

    fn handle(&mut self, command: WorkerCmd) {
        match command {
            WorkerCmd::Attach(queue) => {
                self.drain();
                self.queue = Some(queue);
                lock(&self.core).accepting = true;
            }
            WorkerCmd::Detach => {
                self.drain();
                self.queue = None;
                if let Some(recorder) = &mut lock(&self.core).recorder {
                    recorder.flush();
                }
                self.report();
            }
            WorkerCmd::Clear => self.clear(),
            WorkerCmd::SetConfig(config) => {
                self.buffer = config.buffer;
                self.sensitivity = config.sensitivity;
                if let Some(recorder) = &mut lock(&self.core).recorder {
                    recorder.buffer_mut().set_max_speech(config.buffer);
                    recorder.set_threshold(config.sensitivity.threshold());
                }
                self.report();
            }
            WorkerCmd::Shutdown => {}
        }
    }

    /// Moves everything the capture delivered so far into the recorder, each part in the format it came in.
    fn drain(&mut self) {
        loop {
            let Some(queue) = &mut self.queue else { return };
            // Format changes due where the worker is in the stream.
            while let Ok(change) = queue.formats.peek() {
                if change.at > queue.taken {
                    break;
                }
                queue.format = Some(AudioFormat {
                    sample_rate: change.format.sample_rate,
                    channels: change.format.channels,
                });
                let _ = queue.formats.pop();
            }
            let Some(format) = queue.format else { return };
            let channels = usize::from(format.channels.max(1));
            // Up to the next format change, if one is on its way.
            let next_change = queue.formats.peek().ok().map(|c| (c.at - queue.taken) as usize);
            let available = queue.samples.slots().min(next_change.unwrap_or(usize::MAX));
            let mut n = available - available % channels;
            if n == 0 {
                if next_change.is_none_or(|left| left > available) || available == 0 {
                    return;
                }
                // A partial frame before a change can't be played: skip it.
                n = available;
            }
            let Ok(chunk) = queue.samples.read_chunk(n) else { return };
            let (a, b) = chunk.as_slices();
            self.scratch.clear();
            self.scratch.extend_from_slice(a);
            self.scratch.extend_from_slice(b);
            chunk.commit_all();
            queue.taken += n as u64;
            if n % channels == 0 {
                self.record(format);
            }
            self.scratch.fill(0.0);
        }
    }

    /// Pushes the scratch buffer, in `format`, into the recorder: a new one if the format changed.
    fn record(&mut self, format: AudioFormat) {
        self.ensure_vad();
        let channels = usize::from(format.channels.max(1));
        // In parts, so a backlog (the VAD loading, a slow machine) doesn't keep the player from the buffer for long.
        let part = (format.sample_rate as usize / 1000 * PART_MS).max(1) * channels;
        for samples in self.scratch.chunks(part) {
            let mut core = lock(&self.core);
            if !core.accepting {
                break;
            }
            if core.format != Some(format) {
                // A capture in another format: its audio can't go into the same buffer.
                if let Some(old) = &mut core.recorder {
                    old.clear();
                }
                let vad = Box::new(self.vad.clone().expect("created above"));
                let segmenter = SegmenterConfig {
                    threshold: self.sensitivity.threshold(),
                    ..SegmenterConfig::default()
                };
                core.recorder = Some(Recorder::new(format, vad, segmenter, self.buffer));
                core.format = Some(format);
                core.generation += 1;
            }
            if let Some(recorder) = &mut core.recorder {
                recorder.push(samples);
            }
        }
    }

    fn ensure_vad(&mut self) {
        if self.vad.is_some() {
            return;
        }
        let (vad, fallback): (Box<dyn Vad>, bool) = match self.vad_source.take() {
            Some(VadSource::Given(vad)) => (vad, false),
            Some(VadSource::Silero) | None => match SileroVad::new() {
                Ok(vad) => (Box::new(vad), false),
                Err(e) => {
                    eprintln!("Silero VAD failed to load, using the energy VAD: {e}");
                    (Box::new(EnergyVad::default()), true)
                }
            },
        };
        self.vad = Some(SharedVad(Arc::new(Mutex::new(vad))));
        if fallback {
            self.status.update(|s| s.vad_fallback = true);
        }
    }

    /// Wipes the queue and the recorder.
    fn clear(&mut self) {
        if let Some(mut queue) = self.queue.take() {
            let n = queue.samples.slots();
            if let Ok(chunk) = queue.samples.read_chunk(n) {
                chunk.commit_all();
            }
        }
        self.scratch.fill(0.0);
        if let Some(recorder) = &mut lock(&self.core).recorder {
            recorder.clear();
        }
        self.report();
    }

    fn report(&mut self) {
        let buffered = lock(&self.core)
            .recorder
            .as_ref()
            .map_or(Duration::ZERO, |r| r.buffer().speech_duration());
        self.status.update(|s| s.buffered = buffered);
        self.last_status = Instant::now();
    }
}
