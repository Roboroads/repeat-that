use std::collections::VecDeque;
use std::time::Duration;

/// One contiguous stretch of speech, with the silence around it already trimmed.
#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    /// Increases by one for every utterance pushed, so it stays valid while older utterances are evicted.
    pub id: u64,
    /// Mono samples at the buffer's sample rate.
    pub samples: Vec<f32>,
}

/// In-memory ring of the most recent utterances, capped by the total amount of *speech* it holds.
///
/// The oldest utterances are evicted first once the cap is exceeded. In [`SpeechBuffer::get_back`], `0` is the most
/// recent utterance, `1` the one before it, and so on, matching "press again to step one further back".
#[derive(Debug)]
pub struct SpeechBuffer {
    sample_rate: u32,
    max_samples: usize,
    total_samples: usize,
    next_id: u64,
    utterances: VecDeque<Utterance>,
}

impl SpeechBuffer {
    pub fn new(sample_rate: u32, max_speech: Duration) -> Self {
        Self {
            sample_rate,
            max_samples: duration_to_samples(max_speech, sample_rate),
            total_samples: 0,
            next_id: 0,
            utterances: VecDeque::new(),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Changes the cap, evicting utterances right away if the buffer is now over it.
    pub fn set_max_speech(&mut self, max_speech: Duration) {
        self.max_samples = duration_to_samples(max_speech, self.sample_rate);
        self.evict();
    }

    /// Appends a finished utterance, evicts the oldest ones until the buffer fits its cap again, and returns the new
    /// utterance's id.
    ///
    /// The newest utterance is always kept, even if it alone exceeds the cap.
    pub fn push(&mut self, samples: Vec<f32>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.total_samples += samples.len();
        self.utterances.push_back(Utterance { id, samples });
        self.evict();
        id
    }

    fn evict(&mut self) {
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

    /// Returns the utterance with `id`, if it is still buffered.
    pub fn get(&self, id: u64) -> Option<&Utterance> {
        let oldest = self.oldest_id()?;
        let index = usize::try_from(id.checked_sub(oldest)?).ok()?;
        self.utterances.get(index)
    }

    pub fn oldest_id(&self) -> Option<u64> {
        self.utterances.front().map(|u| u.id)
    }

    pub fn latest_id(&self) -> Option<u64> {
        self.utterances.back().map(|u| u.id)
    }

    pub fn len(&self) -> usize {
        self.utterances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.utterances.is_empty()
    }

    /// Total duration of buffered speech.
    pub fn speech_duration(&self) -> Duration {
        Duration::from_secs_f64(self.total_samples as f64 / self.sample_rate as f64)
    }

    /// Drops all buffered audio, e.g. when capture stops. Ids keep increasing afterwards.
    pub fn clear(&mut self) {
        // Overwrite before freeing so the audio doesn't linger in freed memory.
        for utterance in &mut self.utterances {
            utterance.samples.fill(0.0);
        }
        self.utterances.clear();
        self.total_samples = 0;
    }
}

pub(crate) fn duration_to_samples(duration: Duration, sample_rate: u32) -> usize {
    (duration.as_secs_f64() * sample_rate as f64).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1000;

    fn speech(seconds: f32, value: f32) -> Vec<f32> {
        vec![value; (seconds * RATE as f32) as usize]
    }

    #[test]
    fn steps_back_from_most_recent() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(10));
        buf.push(speech(1.0, 1.0));
        buf.push(speech(1.0, 2.0));
        buf.push(speech(1.0, 3.0));

        assert_eq!(buf.get_back(0).unwrap().samples[0], 3.0);
        assert_eq!(buf.get_back(2).unwrap().samples[0], 1.0);
        assert!(buf.get_back(3).is_none());
        assert!(buf.get_back(usize::MAX).is_none());
    }

    #[test]
    fn evicts_oldest_when_over_cap() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(3));
        for i in 0..5 {
            buf.push(speech(1.0, i as f32));
        }
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.speech_duration(), Duration::from_secs(3));
        assert_eq!(buf.get_back(2).unwrap().samples[0], 2.0);
    }

    #[test]
    fn ids_stay_valid_after_eviction() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(2));
        let ids: Vec<u64> = (0..4).map(|i| buf.push(speech(1.0, i as f32))).collect();
        assert_eq!(ids, [0, 1, 2, 3]);
        assert_eq!(buf.oldest_id(), Some(2));
        assert!(buf.get(1).is_none());
        assert_eq!(buf.get(3).unwrap().samples[0], 3.0);
        assert!(buf.get(4).is_none());
    }

    #[test]
    fn shrinking_cap_evicts() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(10));
        for i in 0..5 {
            buf.push(speech(1.0, i as f32));
        }
        buf.set_max_speech(Duration::from_secs(2));
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn keeps_single_oversized_utterance() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(1));
        buf.push(speech(0.5, 1.0));
        buf.push(speech(2.0, 2.0));
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.get_back(0).unwrap().samples[0], 2.0);
    }

    #[test]
    fn clear_wipes_everything() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(5));
        buf.push(speech(1.0, 1.0));
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.speech_duration(), Duration::ZERO);
        assert_eq!(buf.push(speech(1.0, 1.0)), 1);
    }
}
