use std::time::Duration;

use crate::buffer::SpeechBuffer;
use crate::resample::downmix;
use crate::segmenter::{Segmenter, SegmenterConfig};
use crate::vad::Vad;
use crate::AudioFormat;

/// Turns captured audio into buffered utterances: downmix → voice activity detection → [`SpeechBuffer`].
pub struct Recorder {
    channels: u16,
    segmenter: Segmenter,
    buffer: SpeechBuffer,
    mono: Vec<f32>,
    finished: Vec<Vec<f32>>,
}

impl Recorder {
    pub fn new(format: AudioFormat, vad: Box<dyn Vad>, segmenter: SegmenterConfig, max_speech: Duration) -> Self {
        Self {
            channels: format.channels,
            segmenter: Segmenter::new(format.sample_rate, vad, segmenter),
            buffer: SpeechBuffer::new(format.sample_rate, max_speech),
            mono: Vec::new(),
            finished: Vec::new(),
        }
    }

    /// Feeds interleaved samples in the format given to [`Recorder::new`]. Returns how many utterances finished.
    pub fn push(&mut self, interleaved: &[f32]) -> usize {
        self.mono.clear();
        downmix(interleaved, self.channels, &mut self.mono);
        self.segmenter.push(&self.mono, &mut self.finished);
        let count = self.finished.len();
        for utterance in self.finished.drain(..) {
            self.buffer.push(utterance);
        }
        count
    }

    /// Closes the utterance in progress, so it can be replayed right away.
    pub fn flush(&mut self) {
        if let Some(utterance) = self.segmenter.flush() {
            self.buffer.push(utterance);
        }
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
    use crate::vad::EnergyVad;

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

    #[test]
    fn records_and_replays_speech_without_silence() {
        let mut rec = Recorder::new(
            FORMAT,
            Box::new(EnergyVad::default()),
            SegmenterConfig::default(),
            Duration::from_secs(30),
        );
        let input = [
            stereo(1.0, 0.0),
            stereo(1.0, 0.5),
            stereo(3.0, 0.0),
            stereo(1.0, 0.5),
            stereo(1.0, 0.0),
        ]
        .concat();
        for chunk in input.chunks(960) {
            rec.push(chunk);
        }
        assert_eq!(rec.buffer().len(), 2);

        let mut player = Player::new(FORMAT.sample_rate, PlayerConfig::default());
        player.replay(rec.buffer());
        player.replay(rec.buffer());
        let mut out = vec![0.0; 48_000 * 7];
        let played = player.render(rec.buffer(), &mut out);
        // 7 s of input, 2 s of it speech: replay takes about 2 s plus padding and gaps.
        let seconds = played as f32 / 48_000.0;
        assert!((2.0..3.5).contains(&seconds), "{seconds}");
    }

    #[test]
    fn clear_forgets_everything() {
        let mut rec = Recorder::new(
            FORMAT,
            Box::new(EnergyVad::default()),
            SegmenterConfig::default(),
            Duration::from_secs(30),
        );
        rec.push(&stereo(1.0, 0.5));
        rec.clear();
        rec.flush();
        assert!(rec.buffer().is_empty());
    }
}
