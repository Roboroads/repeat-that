//! The soft beep that says there's nothing to replay. Generated here, so there's no audio asset to ship.

use std::f32::consts::PI;

/// Each of the two tones, fades included.
const TONE_MS: u32 = 80;
/// Fade in and out of each tone, so it doesn't click.
const FADE_MS: u32 = 15;
/// Peak level: about -18 dBFS.
const LEVEL: f32 = 0.125;
/// A falling pair of tones: "nothing there".
const TONES_HZ: [f32; 2] = [880.0, 660.0];

/// The beep as mono samples at `sample_rate`.
pub(crate) fn beep(sample_rate: u32) -> Vec<f32> {
    let len = (sample_rate * TONE_MS / 1000) as usize;
    let fade = (sample_rate * FADE_MS / 1000).max(1) as usize;
    TONES_HZ
        .iter()
        .flat_map(|&hz| {
            (0..len).map(move |i| {
                let edge = i.min(len - 1 - i);
                // Raised-cosine fade.
                let gain = if edge < fade {
                    0.5 - 0.5 * (PI * edge as f32 / fade as f32).cos()
                } else {
                    1.0
                };
                LEVEL * gain * (2.0 * PI * hz * i as f32 / sample_rate as f32).sin()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_soft_tones_of_80_ms() {
        let beep = beep(48_000);
        assert_eq!(beep.len(), 2 * 3840);
        let peak = beep.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let dbfs = 20.0 * peak.log10();
        assert!((-19.0..=-17.0).contains(&dbfs), "{dbfs} dBFS");
        // Faded at both ends of each tone.
        for edge in [0, 3839, 3840, 7679] {
            assert!(beep[edge].abs() < 1e-3, "sample {edge}: {}", beep[edge]);
        }
        let quiet = beep.iter().filter(|s| s.abs() < 1e-3).count();
        assert!(quiet < beep.len() / 10, "{quiet} near-silent samples");
    }

    #[test]
    fn follows_the_sample_rate() {
        assert_eq!(beep(16_000).len(), 2 * 1280);
    }
}
