//! Per-application audio capture for Repeat that?!.
//!
//! Each OS gets its own backend behind [`AppAudioSource`]:
//! - Windows: WASAPI process loopback (Windows 10 2004+), see `WasapiSource`
//! - Linux: PipeWire (planned)
//! - macOS: Core Audio process taps (macOS 14.2+, planned)
//!
//! [`MockSource`] plays audio from memory or a WAV file as if it were an app, so the rest of the app can be developed
//! and tested on any OS.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

pub mod mock;
mod process_tree;
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
pub trait AppAudioSource: Send + Sync {
    /// Applications that can be captured right now.
    fn list_apps(&self) -> Result<Vec<AudioApp>>;

    /// Starts delivering `app`'s audio to `sink` on a background thread.
    fn start(&self, app: &AudioApp, sink: FrameSink) -> Result<Box<dyn CaptureHandle>>;

    /// Ducks `app` to `level` (0.0–1.0) times the volume it had before ducking. `1.0` restores that volume, so the
    /// user's own per-app volume setting is never lost.
    fn set_app_volume(&self, app: &AudioApp, level: f32) -> Result<()>;
}

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
