//! Voice activity detection: deciding, frame by frame, whether someone is talking.

#[cfg(feature = "silero")]
mod silero;

#[cfg(feature = "silero")]
pub use silero::SileroVad;

/// Sample rate every [`Vad`] expects its frames in.
pub const VAD_SAMPLE_RATE: u32 = 16_000;

/// Classifies fixed-size mono frames at [`VAD_SAMPLE_RATE`].
pub trait Vad: Send {
    /// Number of samples [`Vad::speech_probability`] expects per call.
    fn frame_len(&self) -> usize;

    /// Probability (0.0–1.0) that `frame` contains speech. Implementations may keep state between frames.
    fn speech_probability(&mut self, frame: &[f32]) -> f32;

    /// Forgets any state from previous frames.
    fn reset(&mut self);
}

/// Treats anything louder than a fixed level as speech.
///
/// It can't tell speech from typing or music, so it's only meant for tests and as a fallback.
#[derive(Debug, Clone)]
pub struct EnergyVad {
    frame_len: usize,
    threshold_rms: f32,
}

impl EnergyVad {
    pub fn new(threshold_dbfs: f32) -> Self {
        Self {
            frame_len: 512,
            threshold_rms: 10f32.powf(threshold_dbfs / 20.0),
        }
    }
}

impl Default for EnergyVad {
    fn default() -> Self {
        Self::new(-40.0)
    }
}

impl Vad for EnergyVad {
    fn frame_len(&self) -> usize {
        self.frame_len
    }

    fn speech_probability(&mut self, frame: &[f32]) -> f32 {
        if rms(frame) >= self.threshold_rms {
            1.0
        } else {
            0.0
        }
    }

    fn reset(&mut self) {}
}

pub(crate) fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_vad_thresholds() {
        let mut vad = EnergyVad::new(-20.0);
        assert_eq!(vad.speech_probability(&[0.5; 512]), 1.0);
        assert_eq!(vad.speech_probability(&[0.01; 512]), 0.0);
    }
}
