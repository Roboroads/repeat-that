//! Per-application audio capture for Repeat that?!.
//!
//! Each OS gets its own backend behind [`AppAudioSource`]:
//! - Windows: WASAPI process loopback (Windows 10 2004+)
//! - Linux: PipeWire
//! - macOS: Core Audio process taps (macOS 14.2+)
//!
//! Backends are added in later milestones; see `ROADMAP.md`.

use std::fmt;

/// An application that is currently (or was recently) producing audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioApp {
    pub pid: u32,
    /// Human-readable name, e.g. "Discord".
    pub name: String,
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
    fn stop(self: Box<Self>);
}

/// A platform backend that can capture the audio output of a single application.
pub trait AppAudioSource {
    /// Applications that can be captured right now.
    fn list_apps(&self) -> Result<Vec<AudioApp>>;

    /// Starts delivering `app`'s audio to `sink` on a background thread.
    fn start(&self, app: &AudioApp, sink: FrameSink) -> Result<Box<dyn CaptureHandle>>;

    /// Sets `app`'s playback volume (0.0–1.0), used to duck the call during replay.
    fn set_app_volume(&self, app: &AudioApp, volume: f32) -> Result<()>;
}
