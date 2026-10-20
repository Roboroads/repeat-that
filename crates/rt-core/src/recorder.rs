use std::time::Duration;

use crate::AudioFormat;
use crate::buffer::SpeechBuffer;
use crate::resample::downmix;
use crate::segmenter::{Segmenter, SegmenterConfig, UtteranceSink};
use crate::vad::Vad;

/// Turns captured audio into buffered utterances: downmix → voice activity detection → [`SpeechBuffer`].
///
/// The sentence still being spoken is in the buffer as its open utterance, growing as audio comes in, and
/// [`SpeechBuffer::speech_pending`] tells whether speech has started that isn't in the buffer yet.
pub struct Recorder {
    channels: u16,
    segmenter: Segmenter,
    buffer: SpeechBuffer,
    mono: Vec<f32>,
}

/// Writes what the segmenter reports into the buffer, counting finished utterances.
struct BufferSink<'a> {
    buffer: &'a mut SpeechBuffer,
    finished: usize,
}

impl UtteranceSink for BufferSink<'_> {
    fn open(&mut self, samples: &[f32]) {
        self.buffer.open(samples.to_vec());
    }

    fn append(&mut self, samples: &[f32]) {
        self.buffer.append(samples);
    }

    fn split(&mut self, samples: &[f32]) {
        self.finished += usize::from(self.buffer.open_id().is_some());
        self.buffer.open_continuation(samples.to_vec());
    }

    fn close(&mut self) {
        self.finished += usize::from(self.buffer.close().is_some());
    }
}

impl Recorder {
    pub fn new(format: AudioFormat, vad: Box<dyn Vad>, segmenter: SegmenterConfig, max_speech: Duration) -> Self {
        Self {
            channels: format.channels,
            segmenter: Segmenter::new(format.sample_rate, vad, segmenter),
            buffer: SpeechBuffer::new(format.sample_rate, max_speech),
            mono: Vec::new(),
        }
    }

    /// Feeds interleaved samples in the format given to [`Recorder::new`]. Returns how many utterances finished.
    pub fn push(&mut self, interleaved: &[f32]) -> usize {
        self.mono.clear();
        downmix(interleaved, self.channels, &mut self.mono);
        self.buffer.advance_clock(self.mono.len());
        let mut sink = BufferSink {
            buffer: &mut self.buffer,
            finished: 0,
        };
        self.segmenter.push(&self.mono, &mut sink);
        let finished = sink.finished;
        self.buffer.set_speech_pending(self.segmenter.speech_pending());
        finished
    }

    /// Closes the utterance in progress, so it can be replayed right away.
    pub fn flush(&mut self) {
        let mut sink = BufferSink {
            buffer: &mut self.buffer,
            finished: 0,
        };
        self.segmenter.flush(&mut sink);
        self.buffer.set_speech_pending(self.segmenter.speech_pending());
    }

    /// Applies new detection settings from the next VAD frame on, without wiping the buffer or the utterance in
    /// progress.
    pub fn set_segmenter_config(&mut self, config: &SegmenterConfig) {
        self.segmenter.set_config(config);
    }

    /// Changes only the speech threshold (the sensitivity), like [`Recorder::set_segmenter_config`].
    pub fn set_threshold(&mut self, threshold: f32) {
        self.segmenter.set_threshold(threshold);
    }

    pub fn buffer(&self) -> &SpeechBuffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut SpeechBuffer {
        &mut self.buffer
    }

    /// Wipes all buffered and pending audio.
    pub fn clear(&mut self) {
        self.segmenter.reset();
        self.buffer.clear();
        self.mono.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::{Player, PlayerConfig};
    use crate::vad::{EnergyVad, rms};

    const FORMAT: AudioFormat = AudioFormat {
        sample_rate: 48_000,
        channels: 2,
    };

    fn stereo(seconds: f32, amplitude: f32) -> Vec<f32> {
        let frames = (seconds * FORMAT.sample_rate as f32) as usize;
        (0..frames)
            .flat_map(|i| {
                let s = amplitude * (i as f32 * 2.0 * std::f32::consts::PI * 220.0 / 48_000.0).sin();
                [s, s]
            })
            .collect()
    }

    fn recorder() -> Recorder {
        Recorder::new(
            FORMAT,
            Box::new(EnergyVad::default()),
            SegmenterConfig::default(),
            Duration::from_secs(30),
        )
    }

    /// Feeds `input` in 10 ms chunks, like a capture callback.
    fn feed(rec: &mut Recorder, input: &[f32]) {
        for chunk in input.chunks(960) {
            rec.push(chunk);
        }
    }

    #[test]
    fn records_and_replays_speech_without_silence() {
        let mut rec = recorder();
        let input = [
            stereo(1.0, 0.0),
            stereo(1.0, 0.5),
            stereo(3.0, 0.0),
            stereo(1.0, 0.5),
            stereo(1.0, 0.0),
        ]
        .concat();
        feed(&mut rec, &input);
        assert_eq!(rec.buffer().len(), 2);
        assert_eq!(rec.buffer().open_id(), None);

        let mut player = Player::new(FORMAT.sample_rate, PlayerConfig::default());
        player.replay(rec.buffer());
        player.replay(rec.buffer());
        let mut out = vec![0.0; 48_000 * 7];
        let played = player.render(rec.buffer(), &mut out);
        // 7 s of input, 2 s of it speech: replay takes about 2 s plus padding and gaps.
        let seconds = played.written as f32 / 48_000.0;
        assert!(played.ended);
        assert!((2.0..3.5).contains(&seconds), "{seconds}");
    }

    #[test]
    fn publishes_the_sentence_in_progress() {
        let mut rec = recorder();
        feed(&mut rec, &stereo(1.0, 0.0));
        assert!(!rec.buffer().speech_pending());

        // Speech is pending until it has 250 ms, then it's the open utterance and grows while it's spoken.
        feed(&mut rec, &stereo(0.2, 0.5));
        assert!(rec.buffer().speech_pending());
        assert!(rec.buffer().is_empty());
        feed(&mut rec, &stereo(0.2, 0.5));
        assert!(!rec.buffer().speech_pending());
        let id = rec.buffer().open_id().expect("open");
        let len = rec.buffer().get(id).unwrap().samples.len();
        feed(&mut rec, &stereo(0.5, 0.5));
        assert!(rec.buffer().get(id).unwrap().samples.len() > len);
        assert_eq!(rec.buffer().open_id(), Some(id));

        // A long pause closes it.
        let mut finished = 0;
        for chunk in stereo(1.0, 0.0).chunks(960) {
            finished += rec.push(chunk);
        }
        assert_eq!(finished, 1);
        assert_eq!(rec.buffer().open_id(), None);
        assert_eq!(rec.buffer().latest_finished_id(), Some(id));
    }

    #[test]
    fn short_noise_clears_pending_speech() {
        let mut rec = recorder();
        feed(&mut rec, &stereo(0.1, 0.5));
        assert!(rec.buffer().speech_pending());
        feed(&mut rec, &stereo(1.0, 0.0));
        assert!(!rec.buffer().speech_pending());
        assert!(rec.buffer().is_empty());
    }

    #[test]
    fn flush_closes_the_open_utterance() {
        let mut rec = recorder();
        feed(&mut rec, &stereo(1.0, 0.5));
        assert!(rec.buffer().open_id().is_some());
        rec.flush();
        assert_eq!(rec.buffer().open_id(), None);
        assert_eq!(rec.buffer().len(), 1);
    }

    #[test]
    fn clear_forgets_everything() {
        let mut rec = recorder();
        rec.push(&stereo(1.0, 0.5));
        rec.clear();
        rec.flush();
        assert!(rec.buffer().is_empty());
        assert!(!rec.buffer().speech_pending());
    }

    #[test]
    fn keeps_time_since_the_last_utterance() {
        let mut rec = recorder();
        feed(&mut rec, &[stereo(1.0, 0.5), stereo(1.0, 0.0)].concat());
        // The utterance closed after 700 ms of the pause.
        let age = rec.buffer().since_last_finished().unwrap().as_secs_f32();
        assert!((0.25..0.35).contains(&age), "{age}");
        feed(&mut rec, &stereo(2.0, 0.0));
        let age = rec.buffer().since_last_finished().unwrap().as_secs_f32();
        assert!((2.25..2.35).contains(&age), "{age}");
    }

    #[test]
    fn clear_and_flush_end_pending_speech() {
        let mut rec = recorder();
        feed(&mut rec, &stereo(0.1, 0.5));
        assert!(rec.buffer().speech_pending());
        rec.clear();
        assert!(!rec.buffer().speech_pending());

        feed(&mut rec, &stereo(0.1, 0.5));
        assert!(rec.buffer().speech_pending());
        rec.flush();
        assert!(!rec.buffer().speech_pending());

        // Wiping just the buffer doesn't stop the speech the recorder is hearing.
        feed(&mut rec, &stereo(0.1, 0.5));
        rec.buffer_mut().clear();
        assert!(!rec.buffer().speech_pending());
        feed(&mut rec, &stereo(0.05, 0.5));
        assert!(rec.buffer().speech_pending());
    }

    /// Reports the frame's level as its speech probability, so thresholds between 0 and 1 matter.
    struct LevelVad;

    impl Vad for LevelVad {
        fn frame_len(&self) -> usize {
            512
        }

        fn speech_probability(&mut self, frame: &[f32]) -> f32 {
            (rms(frame) * 2.0).min(1.0)
        }

        fn reset(&mut self) {}
    }

    #[test]
    fn threshold_change_applies_to_later_audio_and_keeps_the_buffer() {
        let mut rec = Recorder::new(
            FORMAT,
            Box::new(LevelVad),
            SegmenterConfig::default(),
            Duration::from_secs(30),
        );
        // Loud speech (probability 0.71) is detected at the default threshold of 0.5, quiet speech (0.42) isn't.
        feed(
            &mut rec,
            &[stereo(1.0, 0.5), stereo(1.0, 0.0), stereo(1.0, 0.3), stereo(1.0, 0.0)].concat(),
        );
        assert_eq!(rec.buffer().len(), 1);

        rec.set_threshold(0.35);
        feed(&mut rec, &[stereo(1.0, 0.3), stereo(1.0, 0.0)].concat());
        assert_eq!(rec.buffer().len(), 2);
        assert_eq!(rec.buffer().latest_finished_id(), Some(1));

        rec.set_segmenter_config(&SegmenterConfig::default());
        feed(&mut rec, &[stereo(1.0, 0.3), stereo(1.0, 0.0)].concat());
        assert_eq!(rec.buffer().len(), 2);
    }
}
