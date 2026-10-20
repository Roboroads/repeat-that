//! The worker thread: drains captured audio from the capture's queue into the [`Recorder`].

use std::sync::atomic::{AtomicU64, Ordering};
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
}

pub(crate) type SharedCore = Arc<Mutex<Core>>;

pub(crate) fn lock(core: &SharedCore) -> std::sync::MutexGuard<'_, Core> {
    core.lock().unwrap_or_else(|e| e.into_inner())
}

/// The capture side of a capture's queue: interleaved samples, and their format once the first ones came in.
pub(crate) struct CaptureQueue {
    pub(crate) consumer: rtrb::Consumer<f32>,
    pub(crate) format: Arc<AtomicU64>,
}

/// A format packed into an `AtomicU64`, `0` before the first frames.
pub(crate) fn pack(format: CaptureFormat) -> u64 {
    (u64::from(format.sample_rate) << 16) | u64::from(format.channels)
}

fn unpack(packed: u64) -> Option<AudioFormat> {
    (packed != 0).then_some(AudioFormat {
        sample_rate: (packed >> 16) as u32,
        channels: (packed & 0xffff) as u16,
    })
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

    /// Moves everything the capture delivered so far into the recorder.
    fn drain(&mut self) {
        let Some(queue) = &mut self.queue else { return };
        let Some(format) = unpack(queue.format.load(Ordering::Acquire)) else {
            return;
        };
        let channels = usize::from(format.channels.max(1));
        let available = queue.consumer.slots();
        let n = available - available % channels;
        if n == 0 {
            return;
        }
        let Ok(chunk) = queue.consumer.read_chunk(n) else {
            return;
        };
        let (a, b) = chunk.as_slices();
        self.scratch.clear();
        self.scratch.extend_from_slice(a);
        self.scratch.extend_from_slice(b);
        chunk.commit_all();

        self.ensure_vad();
        // In parts, so a backlog (the VAD loading, a slow machine) doesn't keep the player from the buffer for long.
        let part = (format.sample_rate as usize / 1000 * PART_MS).max(1) * channels;
        for samples in self.scratch.chunks(part) {
            let mut core = lock(&self.core);
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
        self.scratch.fill(0.0);
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
            let n = queue.consumer.slots();
            if let Ok(chunk) = queue.consumer.read_chunk(n) {
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
