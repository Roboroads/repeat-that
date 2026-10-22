//! The control thread: which app to capture, starting and stopping its capture, and reconnecting when it comes back.
//!
//! Listing apps and starting a capture talk to the OS audio service and can block for seconds, so they run here and
//! nowhere near the player.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use rt_capture::{AppAudioSource, AudioApp, CaptureError, CaptureFormat, CaptureHandle, FrameSink};

use crate::duck::DuckCmd;
use crate::playback::PlayCmd;
use crate::status::{CaptureState, StatusHub};
use crate::worker::{CaptureQueue, FormatChange, SharedCore, WorkerCmd};
use crate::{TargetApp, lock};

/// How often a missing target is looked for.
const POLL_EVERY: Duration = Duration::from_secs(2);
/// How often a running capture is checked.
const CHECK_EVERY: Duration = Duration::from_millis(100);
/// The capture queue holds this much audio at 48 kHz stereo, so the worker can fall behind for a while.
const QUEUE_LEN: usize = 48_000 * 2 * 2;
/// Format changes the worker can fall behind on. Audio in yet another format is dropped until it catches up.
const FORMAT_CHANGES: usize = 16;

pub(crate) enum ControlCmd {
    SetTarget(Option<TargetApp>),
    SetPaused(bool),
    Shutdown,
}

pub(crate) struct Control {
    source: Arc<dyn AppAudioSource>,
    core: SharedCore,
    worker: Sender<WorkerCmd>,
    playback: Sender<PlayCmd>,
    duck: Sender<DuckCmd>,
    status: Arc<StatusHub>,
    dropped: Arc<AtomicU64>,
    target: Option<TargetApp>,
    paused: bool,
    capture: Option<Box<dyn CaptureHandle>>,
    /// Capture isn't supported here, so there's no point in polling until the target or pause changes.
    unsupported: bool,
    next_poll: Instant,
}

impl Control {
    pub(crate) fn new(
        source: Arc<dyn AppAudioSource>,
        core: SharedCore,
        worker: Sender<WorkerCmd>,
        playback: Sender<PlayCmd>,
        duck: Sender<DuckCmd>,
        status: Arc<StatusHub>,
        dropped: Arc<AtomicU64>,
    ) -> Self {
        Self {
            source,
            core,
            worker,
            playback,
            duck,
            status,
            dropped,
            target: None,
            paused: false,
            capture: None,
            unsupported: false,
            next_poll: Instant::now(),
        }
    }

    pub(crate) fn run(mut self, commands: Receiver<ControlCmd>) {
        loop {
            let timeout = if self.capture.is_some() {
                CHECK_EVERY
            } else if self.wants_capture() {
                self.next_poll
                    .saturating_duration_since(Instant::now())
                    .min(CHECK_EVERY)
            } else {
                Duration::from_secs(3600)
            };
            match commands.recv_timeout(timeout) {
                Ok(ControlCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(ControlCmd::SetTarget(target)) => self.set_target(target),
                Ok(ControlCmd::SetPaused(paused)) => self.set_paused(paused),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.check_capture();
            if self.wants_capture() && self.capture.is_none() && Instant::now() >= self.next_poll {
                self.poll();
            }
        }
        self.capture = None;
        let _ = self.worker.send(WorkerCmd::Clear);
    }

    fn wants_capture(&self) -> bool {
        self.target.is_some() && !self.paused && !self.unsupported
    }

    fn set_target(&mut self, target: Option<TargetApp>) {
        if target == self.target {
            return;
        }
        self.wipe();
        self.target = target;
        self.unsupported = false;
        self.next_poll = Instant::now();
        let name = self.target.as_ref().map(|t| t.name.clone());
        let state = self.idle_state();
        self.status.update(|s| {
            s.target_name = name;
            s.capture = state;
        });
    }

    fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        if paused {
            self.wipe();
        }
        self.paused = paused;
        self.unsupported = false;
        self.next_poll = Instant::now();
        let state = self.idle_state();
        self.status.update(|s| s.capture = state);
    }

    /// The capture state while not capturing.
    fn idle_state(&self) -> CaptureState {
        match (&self.target, self.paused) {
            (None, _) => CaptureState::NoTarget,
            (Some(_), true) => CaptureState::Paused,
            (Some(_), false) => CaptureState::Waiting,
        }
    }

    /// Stops capture, playback and ducking, and wipes the buffer.
    fn wipe(&mut self) {
        // The buffer goes first, so a replay pressed from now on finds nothing, also while the capture takes its time
        // to stop. The worker drops what the capture still delivers.
        {
            let mut core = lock(&self.core);
            core.accepting = false;
            if let Some(recorder) = &mut core.recorder {
                recorder.clear();
            }
        }
        self.status.update(|s| s.buffered = Duration::ZERO);
        let _ = self.playback.send(PlayCmd::Wipe);
        // The old app gets its volume back before anything else is ducked.
        let _ = self.duck.send(DuckCmd::SetApp(None));
        if let Some(capture) = self.capture.take() {
            capture.stop();
        }
        let _ = self.worker.send(WorkerCmd::Clear);
    }

    /// Notices a capture that ended by itself (the app quit): the buffer stays, and the target is looked for again.
    fn check_capture(&mut self) {
        if self.capture.as_ref().is_none_or(|c| c.is_running()) {
            return;
        }
        if let Some(capture) = self.capture.take() {
            capture.stop();
        }
        let _ = self.worker.send(WorkerCmd::Detach);
        self.next_poll = Instant::now() + POLL_EVERY;
        let state = self.idle_state();
        self.status.update(|s| s.capture = state);
    }

    /// Looks for the target, and starts capturing it if it's there.
    fn poll(&mut self) {
        self.next_poll = Instant::now() + POLL_EVERY;
        let Some(target) = &self.target else { return };
        let state = match self.source.list_apps() {
            Ok(apps) => match apps.iter().find(|app| target.matches(app)) {
                Some(app) => self.start(app.clone()),
                None => CaptureState::Waiting,
            },
            Err(e) => self.failed(e),
        };
        self.status.update(|s| s.capture = state);
    }

    fn start(&mut self, app: AudioApp) -> CaptureState {
        let (samples, samples_out) = rtrb::RingBuffer::new(QUEUE_LEN);
        let (formats, formats_out) = rtrb::RingBuffer::new(FORMAT_CHANGES);
        let sink = sink(samples, formats, Arc::clone(&self.dropped));
        match self.source.start(&app, sink) {
            Ok(capture) => {
                self.capture = Some(capture);
                let _ = self
                    .worker
                    .send(WorkerCmd::Attach(CaptureQueue::new(samples_out, formats_out)));
                let _ = self.duck.send(DuckCmd::SetApp(Some(app)));
                CaptureState::Capturing
            }
            Err(e) => self.failed(e),
        }
    }

    /// The state for a capture that failed with `error`. Only `Unsupported` stops the polling; the rest is tried again,
    /// so a permission the user gives later starts the capture by itself. The buffer is kept either way.
    fn failed(&mut self, error: CaptureError) -> CaptureState {
        match error {
            CaptureError::Unsupported(why) => {
                self.unsupported = true;
                CaptureState::Unsupported(why)
            }
            // Already a sentence for the user.
            CaptureError::PermissionDenied(how) => CaptureState::PermissionDenied(how),
            other => CaptureState::Error(other.to_string()),
        }
    }
}

/// Puts captured frames into the queue, and every change of their format into the format queue, with where in the
/// stream it happens. Runs on the capture thread, so it never blocks: frames that don't fit are dropped and counted.
fn sink(
    mut samples: rtrb::Producer<f32>,
    mut formats: rtrb::Producer<FormatChange>,
    dropped: Arc<AtomicU64>,
) -> FrameSink {
    let mut current = None;
    let mut pushed = 0u64;
    Box::new(move |frames: &[f32], format: CaptureFormat| {
        if current != Some(format) {
            if formats.push(FormatChange { at: pushed, format }).is_err() {
                dropped.fetch_add(frames.len() as u64, Ordering::Relaxed);
                return;
            }
            current = Some(format);
        }
        match samples.push_entire_slice(frames) {
            Ok(()) => pushed += frames.len() as u64,
            Err(_) => _ = dropped.fetch_add(frames.len() as u64, Ordering::Relaxed),
        }
    })
}
