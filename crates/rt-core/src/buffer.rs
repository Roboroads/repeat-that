use std::collections::VecDeque;
use std::time::Duration;

use crate::AudioFormat;

/// One contiguous stretch of speech, with the silence around it already trimmed.
#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    /// Interleaved samples in the buffer's [`AudioFormat`].
    pub samples: Vec<f32>,
}

/// In-memory ring of the most recent utterances, capped by the total amount of *speech* it holds.
///
/// The oldest utterances are evicted first once the cap is exceeded. Index `0` in [`SpeechBuffer::get_back`] is the
/// most recent utterance, `1` the one before it, and so on — matching "press again to step one further back".
#[derive(Debug)]
pub struct SpeechBuffer {
    format: AudioFormat,
    max_samples: usize,
    total_samples: usize,
    utterances: VecDeque<Utterance>,
}

impl SpeechBuffer {
    pub fn new(format: AudioFormat, max_speech: Duration) -> Self {
        let max_samples = (max_speech.as_secs_f64() * format.samples_per_second() as f64) as usize;
        Self {
            format,
            max_samples,
            total_samples: 0,
            utterances: VecDeque::new(),
        }
    }

    pub fn format(&self) -> AudioFormat {
        self.format
    }

    /// Appends a finished utterance and evicts the oldest ones until the buffer fits its cap again.
    ///
    /// The newest utterance is always kept, even if it alone exceeds the cap.
    pub fn push(&mut self, utterance: Utterance) {
        self.total_samples += utterance.samples.len();
        self.utterances.push_back(utterance);
        while self.total_samples > self.max_samples && self.utterances.len() > 1 {
            if let Some(evicted) = self.utterances.pop_front() {
                self.total_samples -= evicted.samples.len();
            }
        }
    }

    /// Returns the utterance `steps_back` positions before the most recent one (`0` = most recent).
    pub fn get_back(&self, steps_back: usize) -> Option<&Utterance> {
        let len = self.utterances.len();
        steps_back
            .checked_add(1)
            .and_then(|n| len.checked_sub(n))
            .and_then(|i| self.utterances.get(i))
    }

    pub fn len(&self) -> usize {
        self.utterances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.utterances.is_empty()
    }

    /// Total duration of buffered speech.
    pub fn speech_duration(&self) -> Duration {
        Duration::from_secs_f64(self.total_samples as f64 / self.format.samples_per_second() as f64)
    }

    /// Drops all buffered audio, e.g. when capture stops.
    pub fn clear(&mut self) {
        self.utterances.clear();
        self.total_samples = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONO_1K: AudioFormat = AudioFormat {
        sample_rate: 1000,
        channels: 1,
    };

    fn utterance(seconds: f32, value: f32) -> Utterance {
        Utterance {
            samples: vec![value; (seconds * 1000.0) as usize],
        }
    }

    #[test]
    fn steps_back_from_most_recent() {
        let mut buf = SpeechBuffer::new(MONO_1K, Duration::from_secs(10));
        buf.push(utterance(1.0, 1.0));
        buf.push(utterance(1.0, 2.0));
        buf.push(utterance(1.0, 3.0));

        assert_eq!(buf.get_back(0).unwrap().samples[0], 3.0);
        assert_eq!(buf.get_back(2).unwrap().samples[0], 1.0);
        assert!(buf.get_back(3).is_none());
        assert!(buf.get_back(usize::MAX).is_none());
    }

    #[test]
    fn evicts_oldest_when_over_cap() {
        let mut buf = SpeechBuffer::new(MONO_1K, Duration::from_secs(3));
        for i in 0..5 {
            buf.push(utterance(1.0, i as f32));
        }
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.speech_duration(), Duration::from_secs(3));
        assert_eq!(buf.get_back(2).unwrap().samples[0], 2.0);
    }

    #[test]
    fn keeps_single_oversized_utterance() {
        let mut buf = SpeechBuffer::new(MONO_1K, Duration::from_secs(1));
        buf.push(utterance(0.5, 1.0));
        buf.push(utterance(2.0, 2.0));
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.get_back(0).unwrap().samples[0], 2.0);
    }

    #[test]
    fn clear_wipes_everything() {
        let mut buf = SpeechBuffer::new(MONO_1K, Duration::from_secs(5));
        buf.push(utterance(1.0, 1.0));
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.speech_duration(), Duration::ZERO);
    }
}
