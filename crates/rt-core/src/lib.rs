//! Platform-independent core of Repeat That: finding speech in captured audio, keeping it in memory, and replaying
//! it.
//!
//! Audio handled here never leaves memory. See `PRIVACY.md`.
//!
//! The flow is: interleaved capture frames → [`Recorder`] (downmix, [`Vad`], [`Segmenter`]) → [`SpeechBuffer`] →
//! [`Player`] (step back, catch-up speed) → output device.

pub mod buffer;
pub mod player;
pub mod recorder;
pub mod resample;
pub mod segmenter;
pub mod stretch;
pub mod vad;

pub use buffer::{SpeechBuffer, Utterance};
pub use player::{Player, PlayerConfig};
pub use recorder::Recorder;
pub use segmenter::{Segmenter, SegmenterConfig};
pub use vad::{EnergyVad, Vad};

#[cfg(feature = "silero")]
pub use vad::SileroVad;

/// Format of captured audio: interleaved `f32` samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}
