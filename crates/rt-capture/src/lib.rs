//! Per-application audio capture for Repeat that?!.
//!
//! Each OS gets its own backend behind [`AppAudioSource`]:
//! - Windows: WASAPI process loopback (Windows 10 2004+), see `WasapiSource`
//! - Linux: PipeWire (planned)
//! - macOS: Core Audio process taps (macOS 14.2+, planned)
//!
//! [`MockSource`] plays audio from memory or a WAV file as if it were an app, so the rest of the app can be developed
//! and tested on any OS.

use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

mod before_duck;
pub mod duck_guard;
mod duck_journal;
pub mod mock;
mod process_tree;
mod ramp;
#[cfg(windows)]
mod windows;

pub use mock::MockSource;
#[cfg(windows)]
pub use windows::WasapiSource;

/// An application that is currently (or was recently) producing audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioApp {
    /// The process to capture. Backends that capture a process tree use the top-most process of the app.
    pub pid: u32,
    /// Human-readable name, e.g. "Discord".
    pub name: String,
    /// The executable, if known. The UI takes the app's icon from it.
    pub exe_path: Option<PathBuf>,
}

/// Format of the frames a capture backend delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// Receives interleaved `f32` frames from the capture thread. Must not block.
pub type FrameSink = Box<dyn FnMut(&[f32], CaptureFormat) + Send + 'static>;

#[derive(Debug)]
pub enum CaptureError {
    /// This OS or OS version has no supported backend.
    Unsupported(String),
    /// The target application is gone.
    AppNotFound(u32),
    /// The OS audio API returned an error.
    Backend(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(why) => write!(f, "audio capture is not supported here: {why}"),
            Self::AppNotFound(pid) => write!(f, "no audio-producing application with PID {pid}"),
            Self::Backend(msg) => write!(f, "audio backend error: {msg}"),
        }
    }
}

impl std::error::Error for CaptureError {}

pub type Result<T> = std::result::Result<T, CaptureError>;

/// A running capture. Calling [`CaptureHandle::stop`] or dropping it ends the capture.
pub trait CaptureHandle: Send {
    /// Whether audio is still being delivered. Capture ends by itself when the OS stream fails, for example because the
    /// audio service restarted.
    fn is_running(&self) -> bool;

    fn stop(self: Box<Self>);
}

/// A platform backend that can capture the audio output of a single application.
///
/// [`list_apps`](Self::list_apps), [`start`](Self::start) and [`set_app_volume`](Self::set_app_volume) talk to the
/// OS audio service and can block, `start` for up to a few seconds while Windows activates the stream. Don't call
/// them on a UI thread.
pub trait AppAudioSource: Send + Sync {
    /// Applications that can be captured right now.
    fn list_apps(&self) -> Result<Vec<AudioApp>>;

    /// Starts delivering `app`'s audio to `sink` on a background thread. Blocks until the stream runs.
    fn start(&self, app: &AudioApp, sink: FrameSink) -> Result<Box<dyn CaptureHandle>>;

    /// Ducks `app` to `level` times the volume it had before ducking. `1.0` restores that volume, so the user's own
    /// per-app volume setting is never lost. Levels are clamped to [`MIN_DUCK_LEVEL`]–1.0.
    ///
    /// Captured audio keeps its full level while the app is ducked: backends whose capture is affected by the app's
    /// volume (Windows) compensate for it. Those fade the volume in half-dB steps, so the compensation can't be
    /// audibly out of step with it, and return once the fade is done: about 240 ms for a duck to 25%, up to 520 ms
    /// for the deepest.
    fn set_app_volume(&self, app: &AudioApp, level: f32) -> Result<()>;
}

/// The lowest duck level. Ducking never mutes an app completely, because muted audio can't be captured.
pub const MIN_DUCK_LEVEL: f32 = 0.05;

/// The capture backend for this OS.
pub fn platform_source() -> Result<Box<dyn AppAudioSource>> {
    #[cfg(windows)]
    {
        Ok(Box::new(WasapiSource::new()))
    }
    #[cfg(not(windows))]
    {
        Err(CaptureError::Unsupported(format!(
            "no capture backend for {} yet",
            std::env::consts::OS
        )))
    }
}

/// A capture running on its own thread, stopped through a shared flag.
pub(crate) struct ThreadCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ThreadCapture {
    pub(crate) fn new(stop: Arc<AtomicBool>, thread: JoinHandle<()>) -> Self {
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl CaptureHandle for ThreadCapture {
    fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    fn stop(self: Box<Self>) {
        // Drop does the work.
    }
}

impl Drop for ThreadCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A duck level shared between [`AppAudioSource::set_app_volume`] and capture threads, with its recent history so a
/// capture can give each frame the gain for the level it was played at.
#[derive(Clone)]
pub(crate) struct DuckLevel(Arc<Mutex<VecDeque<DuckChange>>>);

/// From `since` on, the app played at `level`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DuckChange {
    /// In the backend's clock (100 ns units on Windows).
    pub since: u64,
    pub level: f32,
}

/// 640 ms of fade steps: well past the oldest audio a capture still has to drain, its 200 ms buffer plus the time a
/// volume change takes to reach the audio.
const DUCK_HISTORY: usize = 64;

impl DuckLevel {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(VecDeque::from([DuckChange {
            since: 0,
            level: 1.0,
        }]))))
    }

    /// The latest level.
    pub(crate) fn get(&self) -> f32 {
        self.lock().back().map_or(1.0, |c| c.level)
    }

    /// Records that the app plays at `level` (clamped with [`clamp_duck_level`]) from `since` on.
    pub(crate) fn set(&self, level: f32, since: u64) {
        let mut changes = self.lock();
        // Changes are kept in time order; a change can't take effect before one that was made earlier.
        let since = changes.back().map_or(since, |c| since.max(c.since));
        if changes.len() == DUCK_HISTORY {
            changes.pop_front();
        }
        changes.push_back(DuckChange {
            since,
            level: clamp_duck_level(level),
        });
    }

    /// Copies the history, oldest first, into `out`.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn history(&self, out: &mut Vec<DuckChange>) {
        out.clear();
        out.extend(self.lock().iter().copied());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<DuckChange>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `level` limited to [`MIN_DUCK_LEVEL`]–1.0. NaN means "not ducked".
pub(crate) fn clamp_duck_level(level: f32) -> f32 {
    if level.is_nan() {
        1.0
    } else {
        level.clamp(MIN_DUCK_LEVEL, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duck_history_stays_in_order_and_bounded() {
        let level = DuckLevel::new();
        level.set(0.5, 100);
        // A later change can't take effect before an earlier one.
        level.set(0.25, 50);
        let mut history = Vec::new();
        level.history(&mut history);
        assert_eq!(
            history.last(),
            Some(&DuckChange {
                since: 100,
                level: 0.25
            })
        );
        assert!(history.windows(2).all(|w| w[0].since <= w[1].since));
        for i in 0..200 {
            level.set(1.0, 1_000 + i);
        }
        level.history(&mut history);
        assert_eq!(history.len(), DUCK_HISTORY);
        assert_eq!(level.get(), 1.0);
    }
}
