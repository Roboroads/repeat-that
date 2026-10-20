use std::collections::VecDeque;
use std::time::Duration;

use crate::buffer::duration_to_samples;
use crate::resample::LinearResampler;
use crate::vad::{VAD_SAMPLE_RATE, Vad};

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
    /// Speech shorter than this (clicks, coughs) is dropped. Once speech reaches it, the utterance opens.
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

/// What the [`Segmenter`] found in the audio fed to it, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum SegmentEvent {
    /// Speech reached `min_speech`: a new utterance opens with these samples, padding included.
    Open(Vec<f32>),
    /// More audio for the open utterance.
    Append(Vec<f32>),
    /// The open utterance ended (a long pause, a split or a flush). Nothing more is appended to it.
    Close,
}

/// Splits a continuous mono stream into utterances, dropping the silence between them.
///
/// Utterances are reported while they are spoken: one opens as soon as it has `min_speech` of speech, and audio is
/// appended to it as it comes in. Silence inside a pause is held back until it is known whether speech resumes (the
/// silence stays) or the utterance ends (it is cut to `padding`), so nothing reported is ever taken back.
///
/// Detection runs on a 16 kHz copy of the stream, while the utterances keep the original sample rate.
pub struct Segmenter {
    vad: Box<dyn Vad>,
    sample_rate: u32,
    threshold: f32,
    hysteresis: f32,
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
    /// While pending: the speech so far, starting with the preroll, not yet reported.
    pending: Vec<f32>,
    /// While pending: speech in `pending`, and the silence at its end.
    speech_samples: usize,
    trailing_silence: usize,
    /// While open: silence since the last speech frame, held back.
    held: Vec<f32>,
    /// While open: samples reported for the open utterance so far.
    open_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Silent,
    /// Speech that hasn't reached `min_speech` yet.
    Pending,
    /// An utterance is open.
    Open,
}

impl Segmenter {
    pub fn new(sample_rate: u32, vad: Box<dyn Vad>, config: SegmenterConfig) -> Self {
        let mut segmenter = Self {
            vad,
            sample_rate,
            threshold: 0.0,
            hysteresis: 0.0,
            padding: 0,
            max_pause: 0,
            min_speech: 0,
            max_utterance: 1,
            ratio: sample_rate as f64 / VAD_SAMPLE_RATE as f64,
            resampler: LinearResampler::new(sample_rate, VAD_SAMPLE_RATE),
            vad_pending: Vec::new(),
            vad_frames_done: 0,
            raw: VecDeque::new(),
            raw_start: 0,
            state: State::Silent,
            preroll: VecDeque::new(),
            pending: Vec::new(),
            speech_samples: 0,
            trailing_silence: 0,
            held: Vec::new(),
            open_len: 0,
        };
        segmenter.set_config(&config);
        segmenter
    }

    /// Applies a new configuration from the next VAD frame on, keeping the utterance in progress and everything
    /// reported so far.
    pub fn set_config(&mut self, config: &SegmenterConfig) {
        let samples = |d| duration_to_samples(d, self.sample_rate);
        self.threshold = config.threshold;
        self.hysteresis = config.hysteresis;
        self.padding = samples(config.padding);
        self.max_pause = samples(config.max_pause);
        self.min_speech = samples(config.min_speech);
        self.max_utterance = samples(config.max_utterance).max(1);
    }

    /// Changes the speech threshold from the next VAD frame on, keeping the hysteresis.
    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold;
    }

    /// Whether speech has started that isn't long enough to be reported yet. It either opens an utterance once it
    /// reaches `min_speech`, or is dropped.
    pub fn speech_pending(&self) -> bool {
        self.state == State::Pending
    }

    /// Feeds mono samples and appends what happened to `events`.
    pub fn push(&mut self, samples: &[f32], events: &mut Vec<SegmentEvent>) {
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
                State::Pending | State::Open => self.threshold - self.hysteresis,
            };
            self.on_frame(chunk, probability >= threshold, events);
        }
        self.vad_pending.drain(..consumed);
    }

    /// Ends the open utterance, if any, e.g. when capture stops. Pending speech too short to report is dropped.
    pub fn flush(&mut self, events: &mut Vec<SegmentEvent>) {
        if self.state == State::Open {
            if self.held.is_empty() {
                // Samples too recent to fill a whole VAD frame belong to the ongoing utterance.
                let rest: Vec<f32> = self.raw.drain(..).collect();
                if !rest.is_empty() {
                    events.push(SegmentEvent::Append(rest));
                }
                events.push(SegmentEvent::Close);
            } else {
                self.close(events);
            }
        }
        self.reset();
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
        self.pending.clear();
        self.speech_samples = 0;
        self.trailing_silence = 0;
        self.held.clear();
        self.open_len = 0;
    }

    fn on_frame(&mut self, chunk: Vec<f32>, is_speech: bool, events: &mut Vec<SegmentEvent>) {
        match self.state {
            State::Silent if is_speech => {
                self.pending.clear();
                self.pending.extend(self.preroll.drain(..));
                self.pending.extend_from_slice(&chunk);
                self.speech_samples = chunk.len();
                self.trailing_silence = 0;
                self.state = State::Pending;
                self.open_if_long_enough(events);
            }
            State::Silent => self.add_preroll(&chunk),
            State::Pending => {
                self.pending.extend_from_slice(&chunk);
                if is_speech {
                    self.speech_samples += chunk.len();
                    self.trailing_silence = 0;
                    self.open_if_long_enough(events);
                } else {
                    self.trailing_silence += chunk.len();
                    if self.trailing_silence >= self.max_pause {
                        // Too little speech: a click or a cough. Drop it.
                        let tail = self.pending.split_off(self.pending.len() - self.trailing_silence);
                        self.pending.clear();
                        self.preroll.clear();
                        self.add_preroll(&tail);
                        self.state = State::Silent;
                    }
                }
            }
            State::Open if is_speech => {
                let mut samples = std::mem::take(&mut self.held);
                samples.extend_from_slice(&chunk);
                self.open_len += samples.len();
                events.push(SegmentEvent::Append(samples));
                self.split_if_too_long(events);
            }
            State::Open => {
                self.held.extend_from_slice(&chunk);
                if self.held.len() >= self.max_pause {
                    self.close(events);
                }
            }
        }
    }

    fn open_if_long_enough(&mut self, events: &mut Vec<SegmentEvent>) {
        if self.speech_samples < self.min_speech {
            return;
        }
        // Called right after a speech frame, so there's no trailing silence to hold back.
        let samples = std::mem::take(&mut self.pending);
        self.open_len = samples.len();
        events.push(SegmentEvent::Open(samples));
        self.held.clear();
        self.state = State::Open;
        self.split_if_too_long(events);
    }

    fn split_if_too_long(&mut self, events: &mut Vec<SegmentEvent>) {
        if self.open_len >= self.max_utterance {
            // Split mid-speech. The next part starts right where this one ends, and is open at once so a replay
            // following the speech doesn't stop in between.
            events.push(SegmentEvent::Close);
            events.push(SegmentEvent::Open(Vec::new()));
            self.open_len = 0;
        }
    }

    /// Closes the open utterance after a pause, keeping `padding` of the held silence.
    fn close(&mut self, events: &mut Vec<SegmentEvent>) {
        let mut silence = std::mem::take(&mut self.held);
        let tail = silence.split_off(silence.len().min(self.padding));
        if !silence.is_empty() {
            events.push(SegmentEvent::Append(silence));
        }
        events.push(SegmentEvent::Close);
        self.preroll.clear();
        self.add_preroll(&tail);
        self.open_len = 0;
        self.state = State::Silent;
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
    /// One VAD frame (32 ms) at `RATE`.
    const FRAME: usize = 1536;

    fn tone(seconds: f32) -> Vec<f32> {
        let n = (seconds * RATE as f32) as usize;
        (0..n)
            .map(|i| 0.5 * (i as f32 * 2.0 * std::f32::consts::PI * 220.0 / RATE as f32).sin())
            .collect()
    }

    fn silence(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * RATE as f32) as usize]
    }

    fn secs(samples: usize) -> f32 {
        samples as f32 / RATE as f32
    }

    fn segmenter(config: SegmenterConfig) -> Segmenter {
        Segmenter::new(RATE, Box::new(EnergyVad::default()), config)
    }

    /// Plays `events` into a list of utterances, each with whether it was closed.
    fn utterances(events: &[SegmentEvent]) -> Vec<(Vec<f32>, bool)> {
        let mut out: Vec<(Vec<f32>, bool)> = Vec::new();
        for event in events {
            match event {
                SegmentEvent::Open(samples) => out.push((samples.clone(), false)),
                SegmentEvent::Append(samples) => {
                    let (utterance, closed) = out.last_mut().expect("append to an open utterance");
                    assert!(!*closed, "append after close");
                    utterance.extend_from_slice(samples);
                }
                SegmentEvent::Close => {
                    let (_, closed) = out.last_mut().expect("close an open utterance");
                    assert!(!*closed, "closed twice");
                    *closed = true;
                }
            }
        }
        out
    }

    /// Segments `parts` in odd-sized chunks, like a real capture callback would deliver them, then flushes.
    fn segment(parts: &[Vec<f32>], config: SegmenterConfig) -> Vec<Vec<f32>> {
        let mut seg = segmenter(config);
        let mut events = Vec::new();
        for chunk in parts.concat().chunks(997) {
            seg.push(chunk, &mut events);
        }
        seg.flush(&mut events);
        utterances(&events)
            .into_iter()
            .map(|(samples, closed)| {
                assert!(closed, "flush closes every utterance");
                samples
            })
            .collect()
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

    #[test]
    fn opens_once_speech_reaches_min_speech() {
        let mut seg = segmenter(SegmenterConfig::default());
        let mut events = Vec::new();
        seg.push(&silence(1.0), &mut events);
        assert!(events.is_empty() && !seg.speech_pending());

        // 10 ms chunks: speech is pending from its first VAD frame until it is published at 250 ms.
        let mut opened_at = None;
        let mut pending_seen = false;
        for (i, chunk) in tone(1.0).chunks(480).enumerate() {
            seg.push(chunk, &mut events);
            if events.is_empty() {
                pending_seen |= seg.speech_pending();
            } else if opened_at.is_none() {
                opened_at = Some((i + 1) * 480);
                assert!(!seg.speech_pending());
            }
        }
        assert!(pending_seen);
        let opened_at = secs(opened_at.expect("opened"));
        assert!(
            (0.25..0.25 + 2.0 * secs(FRAME)).contains(&opened_at),
            "opened at {opened_at} s"
        );
        let SegmentEvent::Open(first) = &events[0] else {
            panic!("starts with Open: {:?}", events[0]);
        };
        // The 200 ms of padding before the speech, then the speech so far.
        assert!(
            (0.45..0.45 + 2.0 * secs(FRAME)).contains(&secs(first.len())),
            "{}",
            secs(first.len())
        );
        assert!(
            !events[1..]
                .iter()
                .any(|e| matches!(e, SegmentEvent::Open(_) | SegmentEvent::Close))
        );
    }

    #[test]
    fn short_speech_never_opens_and_clears_pending() {
        let mut seg = segmenter(SegmenterConfig::default());
        let mut events = Vec::new();
        seg.push(&silence(1.0), &mut events);
        seg.push(&tone(0.1), &mut events);
        seg.push(&silence(0.1), &mut events);
        assert!(seg.speech_pending());
        seg.push(&silence(1.0), &mut events);
        assert!(!seg.speech_pending());
        seg.flush(&mut events);
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn holds_back_silence_inside_a_pause() {
        let mut seg = segmenter(SegmenterConfig::default());
        let mut events = Vec::new();
        seg.push(&silence(1.0), &mut events);
        seg.push(&tone(1.0), &mut events);
        seg.push(&silence(0.5), &mut events);
        // Still open, and the pause so far isn't published: it might be cut later.
        let published = utterances(&events);
        assert_eq!(published.len(), 1);
        let (samples, closed) = &published[0];
        assert!(!closed);
        let len = secs(samples.len());
        assert!((1.2 - secs(FRAME)..1.2 + secs(FRAME)).contains(&len), "{len}");

        // Speech resumes: the pause is part of the utterance after all.
        seg.push(&tone(0.5), &mut events);
        let len = secs(utterances(&events)[0].0.len());
        assert!((2.2 - secs(FRAME)..2.2 + secs(FRAME)).contains(&len), "{len}");

        // A long pause closes it, keeping only the padding of it.
        seg.push(&silence(1.0), &mut events);
        let published = utterances(&events);
        assert_eq!(published.len(), 1);
        let (samples, closed) = &published[0];
        assert!(closed);
        let len = secs(samples.len());
        assert!((2.4 - secs(FRAME)..2.4 + secs(FRAME)).contains(&len), "{len}");
    }

    #[test]
    fn split_opens_the_next_utterance_at_once() {
        let config = SegmenterConfig {
            max_utterance: Duration::from_secs(1),
            ..Default::default()
        };
        let mut seg = segmenter(config);
        let mut events = Vec::new();
        seg.push(&tone(1.5), &mut events);
        let close = events.iter().position(|e| *e == SegmentEvent::Close).expect("split");
        assert_eq!(events[close + 1], SegmentEvent::Open(Vec::new()));
        let published = utterances(&events);
        assert_eq!(published.len(), 2);
        assert!(published[0].1 && !published[1].1);
    }

    #[test]
    fn config_change_applies_to_later_frames() {
        let mut seg = segmenter(SegmenterConfig::default());
        let mut events = Vec::new();
        seg.push(&tone(1.0), &mut events);
        // A pause shorter than the default 700 ms closes the utterance once `max_pause` is lowered, without losing it.
        seg.set_config(&SegmenterConfig {
            max_pause: Duration::from_millis(300),
            ..Default::default()
        });
        seg.push(&silence(0.5), &mut events);
        let published = utterances(&events);
        assert_eq!(published.len(), 1);
        assert!(published[0].1);
    }
}
