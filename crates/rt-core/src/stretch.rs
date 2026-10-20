//! Pitch-preserving time-stretching (WSOLA), used to replay faster without the chipmunk effect.

/// Plays `input` back `speed` times faster (or slower) without changing its pitch.
///
/// Uses WSOLA: overlapping windowed frames are re-spaced in time, and each frame is shifted by up to
/// a few milliseconds to line up with the previous one so the waveform stays continuous.
pub fn time_stretch(input: &[f32], speed: f32, sample_rate: u32) -> Vec<f32> {
    let speed = speed.clamp(0.25, 4.0);
    let frame_len = ms_to_samples(30.0, sample_rate).max(16) & !1;
    let hop = frame_len / 2;
    let tolerance = ms_to_samples(10.0, sample_rate) as isize;

    if (speed - 1.0).abs() < 0.01 || input.len() < frame_len * 2 {
        return input.to_vec();
    }

    let window = hann(frame_len);
    let out_len = (input.len() as f32 / speed) as usize;
    let mut out = vec![0.0; out_len + frame_len];

    add_frame(&mut out, 0, &input[..frame_len], &window);
    let mut prev_pos = 0usize;
    let mut k = 1usize;
    loop {
        let out_pos = k * hop;
        let nominal = (out_pos as f32 * speed) as isize;
        // Where the previous frame would naturally continue; the next frame should look like this.
        let natural = prev_pos + hop;
        if natural + frame_len > input.len() || nominal + tolerance + frame_len as isize > input.len() as isize {
            break;
        }

        let template = &input[natural..natural + hop];
        let lo = (nominal - tolerance).max(0) as usize;
        let hi = (nominal + tolerance) as usize;
        let pos = best_match(input, template, lo, hi);

        if out_pos + frame_len > out.len() {
            break;
        }
        add_frame(&mut out, out_pos, &input[pos..pos + frame_len], &window);
        prev_pos = pos;
        k += 1;
    }

    // The last frame's second half is only half-windowed (fading out), so stop at the end of its first half.
    out.truncate((k * hop).min(out_len));
    out
}

/// Finds the start position in `lo..=hi` whose following samples best correlate with `template`.
fn best_match(input: &[f32], template: &[f32], lo: usize, hi: usize) -> usize {
    let corr = |pos: usize, stride: usize| -> f32 {
        let candidate = &input[pos..pos + template.len()];
        candidate.iter().zip(template).step_by(stride).map(|(a, b)| a * b).sum()
    };

    // Coarse search on every 4th position (and every 2nd sample), then refine around the best one.
    let mut best = lo;
    let mut best_corr = f32::NEG_INFINITY;
    for pos in (lo..=hi).step_by(4) {
        let c = corr(pos, 2);
        if c > best_corr {
            best_corr = c;
            best = pos;
        }
    }
    let coarse = best;
    best_corr = f32::NEG_INFINITY;
    for pos in coarse.saturating_sub(3).max(lo)..=(coarse + 3).min(hi) {
        let c = corr(pos, 1);
        if c > best_corr {
            best_corr = c;
            best = pos;
        }
    }
    best
}

fn add_frame(out: &mut [f32], at: usize, frame: &[f32], window: &[f32]) {
    for ((o, s), w) in out[at..at + frame.len()].iter_mut().zip(frame).zip(window) {
        *o += s * w;
    }
}

/// Periodic Hann window: copies spaced half a window apart sum to exactly 1.
fn hann(len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / len as f32).cos())
        .collect()
}

fn ms_to_samples(ms: f32, sample_rate: u32) -> usize {
    (ms / 1000.0 * sample_rate as f32) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vad::rms;

    const RATE: u32 = 48_000;

    fn sine(freq: f32, seconds: f32) -> Vec<f32> {
        (0..(seconds * RATE as f32) as usize)
            .map(|i| 0.5 * (i as f32 * 2.0 * std::f32::consts::PI * freq / RATE as f32).sin())
            .collect()
    }

    fn zero_crossings_per_second(samples: &[f32]) -> f32 {
        let crossings = samples.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        crossings as f32 / (samples.len() as f32 / RATE as f32)
    }

    #[test]
    fn speeds_up_without_changing_pitch() {
        let input = sine(220.0, 3.0);
        for speed in [1.25, 1.5, 2.0] {
            let out = time_stretch(&input, speed, RATE);
            let expected = input.len() as f32 / speed;
            assert!(
                (out.len() as f32 - expected).abs() / expected < 0.03,
                "speed {speed}: len {}",
                out.len()
            );

            // Skip the fade-in of the first frame.
            let body = &out[2000..out.len() - 2000];
            let ratio = zero_crossings_per_second(body) / zero_crossings_per_second(&input);
            assert!((ratio - 1.0).abs() < 0.03, "speed {speed}: pitch ratio {ratio}");
            let level = rms(body) / rms(&input);
            assert!((level - 1.0).abs() < 0.1, "speed {speed}: level ratio {level}");
        }
    }

    #[test]
    fn normal_speed_is_a_copy() {
        let input = sine(220.0, 0.5);
        assert_eq!(time_stretch(&input, 1.0, RATE), input);
    }

    #[test]
    fn short_input_is_returned_as_is() {
        let input = sine(220.0, 0.01);
        assert_eq!(time_stretch(&input, 2.0, RATE), input);
    }
}
