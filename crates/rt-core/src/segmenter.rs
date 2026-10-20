use std::collections::VecDeque;
use std::time::Duration;

use crate::buffer::duration_to_samples;
use crate::resample::LinearResampler;
use crate::vad::{Vad, VAD_SAMPLE_RATE};

#[derive(Debug, Clone)]
pub struct SegmenterConfig {
    /// Speech probability at which a frame starts counting as speech.
    pub threshold: f32,
    /// Once in speech, frames keep counting as speech down to `threshold - hysteresis`.
    pub hysteresis: f32,
    /// Audio kept before speech starts and after it ends, so word edges aren't clipped.
    pub padding: Duration,
    /// Silence longer than this ends the utterance. Shorter pauses stay inside it.
    pub max_pause: Duration,
    /// Utterances with less speech than this (clicks, coughs) are dropped.
    pub min_speech: Duration,
    /// Longer stretches of speech are split, so stepping back stays fine-grained during a monologue.
    pub max_utterance: Duration,
}

impl Default for SegmenterConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            hysteresis: 0.15,
            padding: Duration::from_millis(200),
            max_pause: Duration::from_millis(700),
            min_speech: Duration::from_millis(250),
            max_utterance: Duration::from_secs(15),
        }
    }
}

/// Splits a continuous mono stream into utterances, dropping the silence between them.
///
/// Detection runs on a 16 kHz copy of the stream, while the emitted utterances keep the original sample rate.
pub struct Segmenter {
    vad: Box<dyn Vad>,
    threshold: f32,
    release_threshold: f32,
    padding: usize,
    max_pause: usize,
    min_speech: usize,
    max_utterance: usize,

    /// Original samples per VAD sample.
    ratio: f64,
    resampler: LinearResampler,
    vad_pending: Vec<f32>,
    vad_frames_done: u64,
    /// Original samples not yet assigned to a VAD frame. `raw[0]` has absolute index `raw_start`.
    raw: VecDeque<f32>,
    raw_start: u64,

    state: State,
    /// Most recent audio while not in speech, capped at `padding` samples.
    preroll: VecDeque<f32>,
    current: Vec<f32>,
    speech_samples: usize,
    trailing_silence: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Silent,
    Speech,
}

impl Segmenter {
    pub fn new(sample_rate: u32, vad: Box<dyn Vad>, config: SegmenterConfig) -> Self {
        let samples = |d| duration_to_samples(d, sample_rate);
        Self {
            vad,
            threshold: config.threshold,
            release_threshold: config.threshold - config.hysteresis,
            padding: samples(config.padding),
            max_pause: samples(config.max_pause),
            min_speech: samples(config.min_speech),
            max_utterance: samples(config.max_utterance).max(1),
            ratio: sample_rate as f64 / VAD_SAMPLE_RATE as f64,
            resampler: LinearResampler::new(sample_rate, VAD_SAMPLE_RATE),
            vad_pending: Vec::new(),
            vad_frames_done: 0,
            raw: VecDeque::new(),
            raw_start: 0,
            state: State::Silent,
            preroll: VecDeque::new(),
            current: Vec::new(),
            speech_samples: 0,
            trailing_silence: 0,
        }
    }

    /// Feeds mono samples and appends every utterance that finished to `finished`.
    pub fn push(&mut self, samples: &[f32], finished: &mut Vec<Vec<f32>>) {
        self.raw.extend(samples);
        self.resampler.process(samples, &mut self.vad_pending);

        let frame_len = self.vad.frame_len();
        let mut consumed = 0;
        while self.vad_pending.len() - consumed >= frame_len {
            let probability = self
                .vad
                .speech_probability(&self.vad_pending[consumed..consumed + frame_len]);
            consumed += frame_len;
            self.vad_frames_done += 1;

            // The original samples this frame covers. The resampler lags by under one sample, so clamp to what we have.
            let end = ((self.vad_frames_done * frame_len as u64) as f64 * self.ratio).round() as u64;
            let available = self.raw_start + self.raw.len() as u64;
            let take = (end.min(available) - self.raw_start) as usize;
            let chunk: Vec<f32> = self.raw.drain(..take).collect();
            self.raw_start += take as u64;

            let threshold = match self.state {
                State::Silent => self.threshold,
                State::Speech => self.release_threshold,
            };
            self.on_frame(chunk, probability >= threshold, finished);
        }
        self.vad_pending.drain(..consumed);
    }

    /// Ends the current utterance, if any, e.g. when capture stops.
    pub fn flush(&mut self) -> Option<Vec<f32>> {
        let utterance = match self.state {
            State::Speech => {
                // Samples too recent to fill a whole VAD frame belong to the ongoing utterance.
                self.current.extend(self.raw.drain(..));
                self.finish()
            }
            State::Silent => None,
        };
        self.reset();
        utterance
    }

    /// Drops all pending audio and state.
    pub fn reset(&mut self) {
        self.vad.reset();
        self.resampler.reset();
        self.vad_pending.clear();
        self.vad_frames_done = 0;
        self.raw.clear();
        self.raw_start = 0;
        self.state = State::Silent;
        self.preroll.clear();
        self.current.clear();
        self.speech_samples = 0;
        self.trailing_silence = 0;
    }

    fn on_frame(&mut self, chunk: Vec<f32>, is_speech: bool, finished: &mut Vec<Vec<f32>>) {
        match self.state {
            State::Silent if is_speech => {
                self.current.clear();
                self.current.extend(self.preroll.drain(..));
                self.current.extend_from_slice(&chunk);
                self.speech_samples = chunk.len();
                self.trailing_silence = 0;
                self.state = State::Speech;
            }
            State::Silent => self.add_preroll(&chunk),
            State::Speech => {
                self.current.extend_from_slice(&chunk);
                if is_speech {
                    self.speech_samples += chunk.len();
                    self.trailing_silence = 0;
                } else {
                    self.trailing_silence += chunk.len();
                }

                if self.trailing_silence >= self.max_pause {
                    finished.extend(self.finish());
                    self.state = State::Silent;
                } else if is_speech && self.current.len() >= self.max_utterance {
                    // Split mid-speech. The next part starts right where this one ends.
                    finished.push(std::mem::take(&mut self.current));
                    self.speech_samples = 0;
                }
            }
        }
    }

    /// Takes the current utterance, trimming trailing silence down to `padding`.
    fn finish(&mut self) -> Option<Vec<f32>> {
        let keep_silence = self.trailing_silence.min(self.padding);
        let keep = self.current.len() - self.trailing_silence + keep_silence;
        let tail = self.current.split_off(keep);
        let utterance = std::mem::take(&mut self.current);
        self.preroll.clear();
        self.add_preroll(&tail);
        self.trailing_silence = 0;
        (self.speech_samples >= self.min_speech).then_some(utterance)
    }

    fn add_preroll(&mut self, samples: &[f32]) {
        let skip = samples.len().saturating_sub(self.padding);
        self.preroll.extend(&samples[skip..]);
        let excess = self.preroll.len().saturating_sub(self.padding);
        self.preroll.drain(..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vad::EnergyVad;

    const RATE: u32 = 48_000;

    fn tone(seconds: f32) -> Vec<f32> {
        let n = (seconds * RATE as f32) as usize;
        (0..n)
            .map(|i| 0.5 * (i as f32 * 2.0 * std::f32::consts::PI * 220.0 / RATE as f32).sin())
            .collect()
    }

    fn silence(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * RATE as f32) as usize]
    }

    fn segment(parts: &[Vec<f32>], config: SegmenterConfig) -> Vec<Vec<f32>> {
        let mut seg = Segmenter::new(RATE, Box::new(EnergyVad::default()), config);
        let mut out = Vec::new();
        // Feed in odd-sized chunks, like a real capture callback would.
        for chunk in parts.concat().chunks(997) {
            seg.push(chunk, &mut out);
        }
        out.extend(seg.flush());
        out
    }

    fn secs(samples: usize) -> f32 {
        samples as f32 / RATE as f32
    }

    #[test]
    fn trims_silence_between_utterances() {
        let out = segment(
            &[silence(2.0), tone(1.0), silence(2.0), tone(1.5), silence(2.0)],
            SegmenterConfig::default(),
        );
        assert_eq!(out.len(), 2);
        // Speech plus up to 200 ms of padding on each side (VAD frames are 32 ms, so allow some slack).
        assert!((1.0..1.5).contains(&secs(out[0].len())), "{}", secs(out[0].len()));
        assert!((1.5..2.0).contains(&secs(out[1].len())), "{}", secs(out[1].len()));
    }

    #[test]
    fn short_pauses_stay_in_one_utterance() {
        let out = segment(
            &[tone(1.0), silence(0.3), tone(1.0), silence(2.0)],
            SegmenterConfig::default(),
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn drops_clicks() {
        let out = segment(&[silence(1.0), tone(0.1), silence(2.0)], SegmenterConfig::default());
        assert!(out.is_empty());
    }

    #[test]
    fn splits_long_monologues() {
        let config = SegmenterConfig {
            max_utterance: Duration::from_secs(5),
            ..Default::default()
        };
        let out = segment(&[tone(12.0), silence(2.0)], config);
        assert_eq!(out.len(), 3);
        assert!(out[..2].iter().all(|u| (5.0..5.1).contains(&secs(u.len()))));
    }

    #[test]
    fn flush_emits_unfinished_utterance() {
        let out = segment(&[silence(0.5), tone(1.0)], SegmenterConfig::default());
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn keeps_every_sample_of_continuous_speech() {
        let config = SegmenterConfig {
            max_utterance: Duration::from_secs(1),
            padding: Duration::ZERO,
            ..Default::default()
        };
        let input = tone(4.0);
        let out = segment(std::slice::from_ref(&input), config);
        assert_eq!(out.concat(), input);
    }
}
