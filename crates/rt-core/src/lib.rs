//! Platform-independent core of Repeat that?!: buffering speech in memory and deciding what to replay.
//!
//! Audio handled here never leaves memory — see `PRIVACY.md`.

pub mod buffer;

pub use buffer::{SpeechBuffer, Utterance};

/// Sample format used throughout the core: interleaved `f32` samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioFormat {
    /// Number of interleaved samples per second.
    pub fn samples_per_second(&self) -> usize {
        self.sample_rate as usize * self.channels as usize
    }
}
