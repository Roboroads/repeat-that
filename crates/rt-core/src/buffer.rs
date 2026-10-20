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
///
/// The most recent utterance can be *open*: someone is still talking, and audio is appended to it as it comes in, so a
/// replay can follow the sentence in progress. There is at most one open utterance, it is always the newest, and it is
/// never evicted.
#[derive(Debug)]
pub struct SpeechBuffer {
    sample_rate: u32,
    max_samples: usize,
    total_samples: usize,
    next_id: u64,
    utterances: VecDeque<Utterance>,
    /// Whether the newest utterance is still open.
    open: bool,
    speech_pending: bool,
}

impl SpeechBuffer {
    pub fn new(sample_rate: u32, max_speech: Duration) -> Self {
        Self {
            sample_rate,
            max_samples: duration_to_samples(max_speech, sample_rate),
            total_samples: 0,
            next_id: 0,
            utterances: VecDeque::new(),
            open: false,
            speech_pending: false,
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
    /// utterance's id. An open utterance is closed first.
    ///
    /// The newest utterance is always kept, even if it alone exceeds the cap.
    pub fn push(&mut self, samples: Vec<f32>) -> u64 {
        let id = self.open(samples);
        self.close();
        id
    }

    /// Starts a new open utterance with `samples` and returns its id. The previous one is closed first if it was still
    /// open.
    pub fn open(&mut self, samples: Vec<f32>) -> u64 {
        self.close();
        let id = self.next_id;
        self.next_id += 1;
        self.total_samples += samples.len();
        self.utterances.push_back(Utterance { id, samples });
        self.open = true;
        self.evict();
        id
    }

    /// Appends samples to the open utterance, evicting the oldest finished ones if the buffer is now over its cap.
    /// Does nothing if no utterance is open.
    pub fn append(&mut self, samples: &[f32]) {
        if !self.open {
            return;
        }
        if let Some(utterance) = self.utterances.back_mut() {
            utterance.samples.extend_from_slice(samples);
            self.total_samples += samples.len();
        }
        self.evict();
    }

    /// Finishes the open utterance: nothing more is appended to it. Returns its id, or `None` if none was open.
    pub fn close(&mut self) -> Option<u64> {
        let id = self.open_id();
        self.open = false;
        id
    }

    fn evict(&mut self) {
        // The newest utterance, which is the open one if there is one, always stays.
        while self.total_samples > self.max_samples && self.utterances.len() > 1 {
            if let Some(evicted) = self.utterances.pop_front() {
                self.total_samples -= evicted.samples.len();
            }
        }
    }

    /// Returns the utterance `steps_back` positions before the most recent one (`0` = most recent, which may be the
    /// open one).
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

    /// Id of the most recent utterance, open or finished.
    pub fn latest_id(&self) -> Option<u64> {
        self.utterances.back().map(|u| u.id)
    }

    /// Id of the open utterance, if someone is still talking.
    pub fn open_id(&self) -> Option<u64> {
        if self.open { self.latest_id() } else { None }
    }

    /// Id of the most recent finished utterance.
    pub fn latest_finished_id(&self) -> Option<u64> {
        self.get_back(usize::from(self.open)).map(|u| u.id)
    }

    /// Whether the recorder hears speech that isn't long enough yet to become an utterance. It either becomes the next
    /// open utterance or is dropped (a click, a cough).
    pub fn speech_pending(&self) -> bool {
        self.speech_pending
    }

    pub fn set_speech_pending(&mut self, pending: bool) {
        self.speech_pending = pending;
    }

    pub fn len(&self) -> usize {
        self.utterances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.utterances.is_empty()
    }

    /// Total duration of buffered speech, including the open utterance.
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
        self.open = false;
        self.speech_pending = false;
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

    #[test]
    fn open_utterance_grows_until_closed() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(10));
        let first = buf.push(speech(1.0, 1.0));
        let open = buf.open(speech(0.5, 2.0));
        assert_eq!(open, first + 1);
        assert_eq!(buf.open_id(), Some(open));
        assert_eq!(buf.latest_id(), Some(open));
        assert_eq!(buf.latest_finished_id(), Some(first));

        buf.append(&speech(0.25, 3.0));
        assert_eq!(buf.get(open).unwrap().samples.len(), 750);
        assert_eq!(buf.speech_duration(), Duration::from_millis(1750));

        assert_eq!(buf.close(), Some(open));
        assert_eq!(buf.open_id(), None);
        assert_eq!(buf.latest_finished_id(), Some(open));
        assert_eq!(buf.close(), None);
        // Nothing open: appending does nothing.
        buf.append(&speech(1.0, 4.0));
        assert_eq!(buf.speech_duration(), Duration::from_millis(1750));
    }

    #[test]
    fn only_finished_utterances_without_open_one() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(10));
        assert_eq!(buf.latest_finished_id(), None);
        let open = buf.open(speech(0.5, 1.0));
        assert_eq!(buf.latest_finished_id(), None);
        // Opening or pushing another one closes the open one first.
        let next = buf.open(speech(0.5, 2.0));
        assert_eq!(buf.latest_finished_id(), Some(open));
        let pushed = buf.push(speech(0.5, 3.0));
        assert_eq!(buf.open_id(), None);
        assert_eq!(buf.latest_finished_id(), Some(pushed));
        assert_eq!(buf.get(next).unwrap().samples[0], 2.0);
    }

    #[test]
    fn never_evicts_open_utterance() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(2));
        buf.push(speech(1.0, 1.0));
        let open = buf.open(speech(0.5, 2.0));
        assert_eq!(buf.len(), 2);
        // Growing past the cap evicts finished utterances, never the open one, even when it alone is over the cap.
        buf.append(&speech(2.0, 2.0));
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.open_id(), Some(open));
        buf.append(&speech(1.0, 2.0));
        buf.set_max_speech(Duration::from_secs(1));
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.speech_duration(), Duration::from_millis(3500));
    }

    #[test]
    fn clear_drops_open_utterance_and_pending_speech() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(5));
        buf.push(speech(1.0, 1.0));
        buf.open(speech(1.0, 2.0));
        buf.set_speech_pending(true);
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.open_id(), None);
        assert!(!buf.speech_pending());
        assert_eq!(buf.speech_duration(), Duration::ZERO);
        assert_eq!(buf.open(speech(1.0, 1.0)), 2);
    }
}
