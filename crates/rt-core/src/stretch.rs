//! Pitch-preserving time-stretching (WSOLA), used to replay faster without the chipmunk effect.

/// Speeds a [`Stretcher`] accepts; others are clamped.
const MIN_SPEED: f32 = 0.25;
const MAX_SPEED: f32 = 4.0;

/// Plays audio `speed` times faster (or slower) without changing its pitch, as a stream.
///
/// Uses WSOLA: overlapping windowed frames are re-spaced in time, and each frame is shifted by up to a few
/// milliseconds to line up with the previous one so the waveform stays continuous.
///
/// Input is pushed in chunks of any size and output pulled as far as the input allows. Each output frame (a hop of
/// 15 ms) is placed at the current speed, so [`Stretcher::set_speed`] takes effect at once, and the overlap with the
/// previous frame keeps a speed change free of clicks. At normal speed the output is an exact copy of the input.
#[derive(Debug, Clone)]
pub struct Stretcher {
    speed: f32,
    frame_len: usize,
    hop: usize,
    tolerance: usize,
    window: Vec<f32>,
    /// Input still needed. Positions below are indices into it.
    input: Vec<f32>,
    /// Where the previous frame continues: its second half covers `input[natural..natural + hop]`. Playing on from
    /// here is always seamless.
    natural: usize,
    /// Where the next frame should start to keep to the speed.
    nominal: f64,
    /// Output ready to be pulled: `ready[ready_pos..]`.
    ready: Vec<f32>,
    ready_pos: usize,
    /// No more input for now: the tail that doesn't fill a frame can be played.
    finished: bool,
}

impl Stretcher {
    pub fn new(sample_rate: u32, speed: f32) -> Self {
        let frame_len = ms_to_samples(30.0, sample_rate).max(16) & !1;
        Self {
            speed: speed.clamp(MIN_SPEED, MAX_SPEED),
            frame_len,
            hop: frame_len / 2,
            tolerance: ms_to_samples(10.0, sample_rate),
            window: hann(frame_len),
            input: Vec::new(),
            natural: 0,
            nominal: 0.0,
            ready: Vec::new(),
            ready_pos: 0,
            finished: false,
        }
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Changes the speed from the next frame on, at most one hop (15 ms) from now.
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
    }

    /// Adds input. If [`Stretcher::finish`] was called and its tail not pulled yet, the stream simply goes on.
    pub fn push(&mut self, input: &[f32]) {
        if !input.is_empty() {
            self.input.extend_from_slice(input);
            self.finished = false;
        }
    }

    /// Marks the end of the input so far: [`Stretcher::pull`] also returns the tail that doesn't fill a whole frame,
    /// at normal speed. Input pushed afterwards continues seamlessly from there.
    pub fn finish(&mut self) {
        self.finished = true;
    }

    /// Writes as much output as the input allows into `out` and returns how many samples it wrote.
    pub fn pull(&mut self, out: &mut [f32]) -> usize {
        let mut written = 0;
        while written < out.len() {
            if self.ready_pos == self.ready.len() {
                self.ready.clear();
                self.ready_pos = 0;
                if !self.next_frame() {
                    break;
                }
            }
            let n = (self.ready.len() - self.ready_pos).min(out.len() - written);
            out[written..written + n].copy_from_slice(&self.ready[self.ready_pos..self.ready_pos + n]);
            self.ready_pos += n;
            written += n;
        }
        written
    }

    /// Output still to come from the input pushed so far, in samples. An estimate while stretching.
    pub fn pending(&self) -> usize {
        let unplayed = self.input.len() - self.natural;
        let unplayed = if self.is_normal_speed() {
            unplayed
        } else {
            (unplayed as f64 / self.speed as f64).round() as usize
        };
        self.ready.len() - self.ready_pos + unplayed
    }

    /// Drops all input and output, keeping the speed.
    pub fn reset(&mut self) {
        self.input.clear();
        self.natural = 0;
        self.nominal = 0.0;
        self.ready.clear();
        self.ready_pos = 0;
        self.finished = false;
    }

    fn is_normal_speed(&self) -> bool {
        (self.speed - 1.0).abs() < 0.01
    }

    /// Adds the next hop of output to `ready`. Returns `false` if the input doesn't reach far enough yet.
    fn next_frame(&mut self) -> bool {
        let (hop, natural) = (self.hop, self.natural);
        if self.is_normal_speed() {
            if natural + hop <= self.input.len() {
                self.ready.extend_from_slice(&self.input[natural..natural + hop]);
                self.natural += hop;
                self.nominal += hop as f64;
                self.compact();
                return true;
            }
        } else {
            let nominal = self.nominal as usize;
            let lo = nominal.saturating_sub(self.tolerance);
            let hi = nominal + self.tolerance;
            if hi + self.frame_len <= self.input.len() {
                let pos = best_match(&self.input, &self.input[natural..natural + hop], lo, hi);
                if pos == natural {
                    // The two halves of the window add up to one: this is the input itself.
                    self.ready.extend_from_slice(&self.input[natural..natural + hop]);
                } else {
                    let fade_out = &self.window[hop..];
                    let fade_in = &self.window[..hop];
                    let previous = &self.input[natural..natural + hop];
                    let next = &self.input[pos..pos + hop];
                    self.ready.extend(
                        previous
                            .iter()
                            .zip(fade_out)
                            .zip(next.iter().zip(fade_in))
                            .map(|((p, wp), (n, wn))| p * wp + n * wn),
                    );
                }
                self.natural = pos + hop;
                self.nominal += hop as f64 * self.speed as f64;
                self.compact();
                return true;
            }
        }

        if self.finished && natural < self.input.len() {
            // Playing on from `natural` continues the previous frame seamlessly.
            self.ready.extend_from_slice(&self.input[natural..]);
            self.natural = self.input.len();
            self.nominal = self.natural as f64;
            self.compact();
            return true;
        }
        false
    }

    /// Drops input no later frame can use, once that is at least half of what's kept.
    fn compact(&mut self) {
        let used = self.natural.min((self.nominal as usize).saturating_sub(self.tolerance));
        if used >= self.frame_len && used * 2 >= self.input.len() {
            self.input.drain(..used);
            self.natural -= used;
            self.nominal -= used as f64;
        }
    }
}

/// Plays `input` back `speed` times faster (or slower) without changing its pitch, in one go.
pub fn time_stretch(input: &[f32], speed: f32, sample_rate: u32) -> Vec<f32> {
    let mut stretcher = Stretcher::new(sample_rate, speed);
    stretcher.push(input);
    stretcher.finish();
    let mut out = vec![0.0; stretcher.pending() + stretcher.frame_len];
    let mut len = 0;
    loop {
        len += stretcher.pull(&mut out[len..]);
        if len < out.len() {
            break;
        }
        out.resize(out.len() + stretcher.frame_len, 0.0);
    }
    out.truncate(len);
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

    /// Pushes `input` in chunks of pseudo-random sizes, pulling everything available after each one.
    fn stream(stretcher: &mut Stretcher, input: &[f32], out: &mut Vec<f32>) {
        let mut seed = 12345u32;
        let mut rest = input;
        while !rest.is_empty() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = ((seed >> 16) as usize % 2000 + 1).min(rest.len());
            stretcher.push(&rest[..n]);
            rest = &rest[n..];
            pull_all(stretcher, out);
        }
    }

    fn pull_all(stretcher: &mut Stretcher, out: &mut Vec<f32>) {
        let mut block = [0.0; 441];
        loop {
            let n = stretcher.pull(&mut block);
            out.extend_from_slice(&block[..n]);
            if n < block.len() {
                return;
            }
        }
    }

    /// Largest difference between neighbouring samples.
    fn max_jump(samples: &[f32]) -> f32 {
        samples.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn streaming_at_normal_speed_is_a_copy() {
        let input = sine(220.0, 1.0);
        let mut stretcher = Stretcher::new(RATE, 1.0);
        let mut out = Vec::new();
        stream(&mut stretcher, &input, &mut out);
        assert!(out.len() < input.len());
        stretcher.finish();
        pull_all(&mut stretcher, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn streaming_matches_batch() {
        let input = sine(220.0, 3.0);
        let mut stretcher = Stretcher::new(RATE, 1.5);
        let mut out = Vec::new();
        stream(&mut stretcher, &input, &mut out);
        stretcher.finish();
        pull_all(&mut stretcher, &mut out);

        let frame = ms_to_samples(30.0, RATE);
        let expected = input.len() as f32 / 1.5;
        assert!((out.len() as f32 - expected).abs() <= frame as f32, "len {}", out.len());
        assert_eq!(out, time_stretch(&input, 1.5, RATE));
    }

    #[test]
    fn speed_changes_apply_at_once_without_clicks() {
        let input = sine(220.0, 2.0);
        let mut stretcher = Stretcher::new(RATE, 1.0);
        let mut out = Vec::new();
        let half = input.len() / 2;
        stretcher.push(&input[..half]);
        pull_all(&mut stretcher, &mut out);
        let before = out.len();

        stretcher.set_speed(1.5);
        assert_eq!(stretcher.speed(), 1.5);
        stretcher.push(&input[half..]);
        stretcher.finish();
        pull_all(&mut stretcher, &mut out);

        // The rest plays 1.5x faster from the change on (give or take a frame).
        let frame = ms_to_samples(30.0, RATE) as f32;
        let expected = (input.len() - before) as f32 / 1.5;
        let rest = (out.len() - before) as f32;
        assert!((rest - expected).abs() <= frame, "rest {rest}, expected {expected}");

        // A 220 Hz sine at 0.5 moves at most 0.0144 per sample. A click would jump much further.
        assert!(max_jump(&out) < 0.03, "max jump {}", max_jump(&out));

        // And back to normal speed.
        let mut stretcher = Stretcher::new(RATE, 1.5);
        let mut out = Vec::new();
        stretcher.push(&input[..half]);
        pull_all(&mut stretcher, &mut out);
        stretcher.set_speed(1.0);
        stretcher.push(&input[half..]);
        stretcher.finish();
        pull_all(&mut stretcher, &mut out);
        assert!(max_jump(&out) < 0.03, "max jump {}", max_jump(&out));
    }

    #[test]
    fn finish_flushes_the_tail_and_more_input_continues_it() {
        let input = sine(220.0, 0.5);
        let mut stretcher = Stretcher::new(RATE, 1.5);
        let mut out = Vec::new();
        stretcher.push(&input[..10_000]);
        pull_all(&mut stretcher, &mut out);
        assert!(stretcher.pending() > 0);

        stretcher.finish();
        pull_all(&mut stretcher, &mut out);
        assert_eq!(stretcher.pending(), 0);
        let flushed = out.len();

        // Input after a flush carries on where it left off, without a click.
        stretcher.push(&input[10_000..]);
        stretcher.finish();
        pull_all(&mut stretcher, &mut out);
        assert!(out.len() > flushed);
        assert!(max_jump(&out) < 0.03, "max jump {}", max_jump(&out));
    }

    #[test]
    fn pending_estimates_output_still_to_come() {
        let mut stretcher = Stretcher::new(RATE, 2.0);
        stretcher.push(&sine(220.0, 1.0));
        let pending = stretcher.pending() as f32;
        assert!((pending - RATE as f32 / 2.0).abs() < 100.0, "{pending}");
        stretcher.reset();
        assert_eq!(stretcher.pending(), 0);
        assert_eq!(stretcher.speed(), 2.0);
    }
}
