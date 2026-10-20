/// Mixes interleaved multi-channel audio down to mono by averaging the channels.
pub fn downmix(interleaved: &[f32], channels: u16, out: &mut Vec<f32>) {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    let scale = 1.0 / channels as f32;
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() * scale),
    );
}

/// Streaming linear-interpolation resampler for mono audio.
///
/// This is only meant to feed the voice activity detector. It has no anti-aliasing filter, which is fine for speech
/// detection but not for audio people listen to.
#[derive(Debug, Clone)]
pub struct LinearResampler {
    /// Input samples advanced per output sample.
    step: f64,
    /// Position of the next output sample. `0.0` is `prev`, `1.0` is the first sample of the next input.
    pos: f64,
    prev: f32,
}

impl LinearResampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        Self {
            step: input_rate as f64 / output_rate as f64,
            pos: 0.0,
            prev: 0.0,
        }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let n = input.len();
        while self.pos < n as f64 {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = if i == 0 { self.prev } else { input[i - 1] };
            let b = input[i];
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        if let Some(&last) = input.last() {
            self.prev = last;
            self.pos -= n as f64;
        }
    }

    pub fn reset(&mut self) {
        self.pos = 0.0;
        self.prev = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmixes_stereo() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
        assert_eq!(out, [0.5, 0.5]);
    }

    #[test]
    fn output_length_matches_ratio_across_chunks() {
        let mut r = LinearResampler::new(48_000, 16_000);
        let mut out = Vec::new();
        for _ in 0..100 {
            r.process(&[0.25; 480], &mut out);
        }
        assert_eq!(out.len(), 16_000);
        assert!(out[10..].iter().all(|&s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    fn handles_non_integer_ratio() {
        let mut r = LinearResampler::new(44_100, 16_000);
        let mut out = Vec::new();
        for _ in 0..10 {
            r.process(&[0.0; 4410], &mut out);
        }
        assert!((out.len() as i64 - 16_000).abs() <= 1, "got {}", out.len());
    }
}
