use std::time::Duration;

use crate::buffer::{SpeechBuffer, duration_to_samples};
use crate::stretch::Stretcher;

pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 2.0;

#[derive(Debug, Clone)]
pub struct PlayerConfig {
    /// Silence inserted between replayed utterances so they don't run into each other.
    pub gap: Duration,
    /// Playback speed, `1.0` is normal. Clamped to [`MIN_SPEED`]..=[`MAX_SPEED`].
    pub speed: f32,
    /// A replay starts at the sentence still being spoken once it is this long. Before that it starts at the last
    /// finished utterance, so it doesn't open with a word or two out of context.
    pub min_open_start: Duration,
    /// Once playback has caught up with someone still talking, it waits until this much of their speech has come in
    /// before it plays on, so it doesn't stutter on every few milliseconds that arrive.
    pub live_margin: Duration,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            gap: Duration::from_millis(250),
            speed: 1.0,
            min_open_start: Duration::from_millis(1500),
            live_margin: Duration::from_millis(100),
        }
    }
}

/// What [`Player::render`] did with a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rendered {
    /// Samples of the block that belong to the replay: speech, gaps, and silence while it waits for someone still
    /// talking. The rest of the block is silence.
    pub written: usize,
    /// The replay has caught up with live and ended (or nothing was playing). Waiting for more speech isn't the end.
    pub ended: bool,
}

/// Decides what to replay and renders it, pulling utterances from a [`SpeechBuffer`].
///
/// Playback starts at some utterance and plays through to live: every newer utterance, including ones recorded while
/// replaying and the sentence still being spoken. When it catches up with someone still talking, it follows their
/// sentence as it comes in, in silence while it waits for more. After a finished utterance it also waits while speech
/// is pending ([`SpeechBuffer::speech_pending`]). Once there's nothing left to play, it stops and the listener is back
/// to live audio.
///
/// Speed changes apply mid-utterance, within 15 ms.
#[derive(Debug)]
pub struct Player {
    sample_rate: u32,
    gap: usize,
    speed: f32,
    min_open_start: usize,
    live_margin: usize,
    stretcher: Stretcher,
    playing: Option<Playback>,
}

#[derive(Debug)]
struct Playback {
    id: u64,
    /// Samples of the utterance pushed into the stretcher so far.
    fed: usize,
    /// The whole utterance is in the stretcher: it was finished (or evicted).
    complete: bool,
    /// Caught up with the sentence in progress. The rest of it plays at normal speed: there's nothing left to catch up,
    /// and playing faster would only run out of speech again.
    live: bool,
    phase: Phase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Speech,
    /// Caught up with the sentence in progress: silence until enough of it has come in, or it finished.
    Waiting,
    /// Silence after a finished utterance, with this many samples left.
    Gap(usize),
    /// On to the next utterance, in silence while speech is pending.
    Next,
}

impl Playback {
    /// Pushes what's new of the utterance into the stretcher, and lets the stretcher play out its tail once the
    /// utterance is finished.
    fn feed(&mut self, buffer: &SpeechBuffer, stretcher: &mut Stretcher) {
        if self.complete {
            return;
        }
        if let Some(utterance) = buffer.get(self.id) {
            stretcher.push(utterance.samples.get(self.fed..).unwrap_or_default());
            self.fed = utterance.samples.len();
        }
        if buffer.open_id() != Some(self.id) {
            self.complete = true;
            stretcher.finish();
        }
    }
}

impl Player {
    pub fn new(sample_rate: u32, config: PlayerConfig) -> Self {
        let speed = config.speed.clamp(MIN_SPEED, MAX_SPEED);
        Self {
            sample_rate,
            gap: duration_to_samples(config.gap, sample_rate),
            speed,
            min_open_start: duration_to_samples(config.min_open_start, sample_rate),
            live_margin: duration_to_samples(config.live_margin, sample_rate),
            stretcher: Stretcher::new(sample_rate, speed),
            playing: None,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// Id of the utterance being played.
    pub fn current_id(&self) -> Option<u64> {
        self.playing.as_ref().map(|p| p.id)
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Changes the speed, mid-utterance. Once playback has caught up with the sentence in progress, the rest of that
    /// sentence stays at normal speed.
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        if !self.playing.as_ref().is_some_and(|p| p.live) {
            self.stretcher.set_speed(self.speed);
        }
    }

    /// The replay button. When idle, starts at the sentence in progress if it is at least `min_open_start` long,
    /// otherwise at the last finished utterance (or the sentence in progress if there is none). While replaying, steps
    /// back one utterance from the one playing now. Returns `false` if there was nothing to play.
    pub fn replay(&mut self, buffer: &SpeechBuffer) -> bool {
        let target = match self.current_id() {
            // Already at the oldest one: start it over.
            Some(id) => id.saturating_sub(1).max(buffer.oldest_id().unwrap_or(0)),
            None => match self.start_id(buffer) {
                Some(id) => id,
                None => return false,
            },
        };
        self.load(buffer, target)
    }

    fn start_id(&self, buffer: &SpeechBuffer) -> Option<u64> {
        let open = buffer.open_id();
        let open_len = open.and_then(|id| buffer.get(id)).map_or(0, |u| u.samples.len());
        if open.is_some() && open_len >= self.min_open_start {
            open
        } else {
            buffer.latest_finished_id().or(open)
        }
    }

    pub fn stop(&mut self) {
        self.playing = None;
        self.stretcher.reset();
    }

    /// How long playback takes to reach live if nothing more is said: the rest of the current utterance and every
    /// newer one, with the gaps between them, at the current speed. `None` while idle.
    pub fn behind(&self, buffer: &SpeechBuffer) -> Option<Duration> {
        let playback = self.playing.as_ref()?;
        let at_speed = |samples: usize, speed: f32| (samples as f32 / speed) as usize;
        let gap_after = |id: u64, speed: f32| {
            if buffer.open_id() == Some(id) {
                0
            } else {
                at_speed(self.gap, speed)
            }
        };

        let current_speed = self.stretcher.speed();
        let mut samples = match playback.phase {
            Phase::Speech | Phase::Waiting => {
                let unfed = buffer
                    .get(playback.id)
                    .map_or(0, |u| u.samples.len().saturating_sub(playback.fed));
                self.stretcher.pending() + at_speed(unfed, current_speed) + gap_after(playback.id, current_speed)
            }
            Phase::Gap(left) => left,
            Phase::Next => 0,
        };
        let newer = (playback.id + 1).max(buffer.oldest_id().unwrap_or(0));
        for id in newer..=buffer.latest_id().unwrap_or(0) {
            if let Some(utterance) = buffer.get(id) {
                samples += at_speed(utterance.samples.len(), self.speed) + gap_after(id, self.speed);
            }
        }
        Some(Duration::from_secs_f64(samples as f64 / self.sample_rate as f64))
    }

    /// Fills `out` with the next block of mono samples. Once the replay ends, the rest of `out` is silence.
    pub fn render(&mut self, buffer: &SpeechBuffer, out: &mut [f32]) -> Rendered {
        let mut written = 0;
        while written < out.len() {
            let Some(playback) = &mut self.playing else {
                break;
            };
            match playback.phase {
                Phase::Speech => {
                    playback.feed(buffer, &mut self.stretcher);
                    written += self.stretcher.pull(&mut out[written..]);
                    if written == out.len() {
                        break;
                    }
                    if playback.complete {
                        let gap = (self.gap as f32 / self.stretcher.speed()) as usize;
                        playback.phase = Phase::Gap(gap);
                        continue;
                    }
                    // Caught up with someone still talking: play all of their sentence so far, then wait for more.
                    playback.live = true;
                    self.stretcher.set_speed(1.0);
                    self.stretcher.finish();
                    written += self.stretcher.pull(&mut out[written..]);
                    if written < out.len() {
                        playback.phase = Phase::Waiting;
                    }
                }
                Phase::Waiting => {
                    playback.feed(buffer, &mut self.stretcher);
                    let available = self.stretcher.pending();
                    if playback.complete || (available > 0 && available >= self.live_margin) {
                        playback.phase = Phase::Speech;
                    } else {
                        out[written..].fill(0.0);
                        written = out.len();
                    }
                }
                Phase::Gap(left) => {
                    let n = left.min(out.len() - written);
                    out[written..written + n].fill(0.0);
                    written += n;
                    playback.phase = if n == left { Phase::Next } else { Phase::Gap(left - n) };
                }
                Phase::Next => {
                    let next = playback.id + 1;
                    if buffer.latest_id().is_some_and(|latest| latest >= next) {
                        self.load(buffer, next);
                    } else if buffer.speech_pending() {
                        out[written..].fill(0.0);
                        written = out.len();
                    } else {
                        self.stop();
                    }
                }
            }
        }
        out[written..].fill(0.0);
        Rendered {
            written,
            ended: self.playing.is_none(),
        }
    }

    /// Starts playing utterance `id` (or the oldest one still buffered, if `id` was evicted). Stops if it doesn't
    /// exist.
    fn load(&mut self, buffer: &SpeechBuffer, id: u64) -> bool {
        let id = id.max(buffer.oldest_id().unwrap_or(id));
        if buffer.get(id).is_none() {
            self.stop();
            return false;
        }
        self.stretcher.reset();
        self.stretcher.set_speed(self.speed);
        let mut playback = Playback {
            id,
            fed: 0,
            complete: false,
            live: false,
            phase: Phase::Speech,
        };
        playback.feed(buffer, &mut self.stretcher);
        self.playing = Some(playback);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1000;

    fn buffer_with(values: &[f32]) -> SpeechBuffer {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(60));
        for &v in values {
            buf.push(vec![v; 100]);
        }
        buf
    }

    fn no_gap() -> PlayerConfig {
        PlayerConfig {
            gap: Duration::ZERO,
            live_margin: Duration::ZERO,
            ..Default::default()
        }
    }

    /// Renders 64-sample blocks until playback ends.
    fn render_all(player: &mut Player, buf: &SpeechBuffer) -> Vec<f32> {
        let mut out = Vec::new();
        let mut block = [0.0; 64];
        for _ in 0..10_000 {
            let rendered = player.render(buf, &mut block);
            out.extend_from_slice(&block[..rendered.written]);
            if rendered.ended {
                return out;
            }
        }
        panic!("playback never ended");
    }

    /// Renders `n` samples in 64-sample blocks, checking that playback goes on.
    fn render_playing(player: &mut Player, buf: &SpeechBuffer, n: usize) -> Vec<f32> {
        let mut out = vec![0.0; n];
        for block in out.chunks_mut(64) {
            let rendered = player.render(buf, block);
            assert_eq!(rendered.written, block.len());
            assert!(!rendered.ended, "ended early");
        }
        out
    }

    fn non_silent(samples: &[f32]) -> Vec<f32> {
        samples.iter().copied().filter(|&s| s != 0.0).collect()
    }

    #[test]
    fn replays_latest_then_stops() {
        let buf = buffer_with(&[1.0, 2.0, 3.0]);
        let mut player = Player::new(RATE, no_gap());
        assert!(player.replay(&buf));
        let out = render_all(&mut player, &buf);
        assert_eq!(out, vec![3.0; 100]);
        assert!(!player.is_playing());
    }

    #[test]
    fn pressing_again_steps_back_and_plays_through_to_live() {
        let buf = buffer_with(&[1.0, 2.0, 3.0]);
        let mut player = Player::new(RATE, no_gap());
        player.replay(&buf);
        player.replay(&buf);
        player.replay(&buf);
        assert_eq!(player.current_id(), Some(0));
        let out = render_all(&mut player, &buf);
        assert_eq!(out, [vec![1.0; 100], vec![2.0; 100], vec![3.0; 100]].concat());
    }

    #[test]
    fn stepping_back_past_oldest_restarts_oldest() {
        let buf = buffer_with(&[1.0, 2.0]);
        let mut player = Player::new(RATE, no_gap());
        for _ in 0..5 {
            player.replay(&buf);
        }
        assert_eq!(player.current_id(), Some(0));
    }

    #[test]
    fn picks_up_utterances_recorded_during_replay() {
        let mut buf = buffer_with(&[1.0]);
        let mut player = Player::new(RATE, no_gap());
        player.replay(&buf);
        let mut block = [0.0; 50];
        player.render(&buf, &mut block);
        buf.push(vec![2.0; 100]);
        let out = render_all(&mut player, &buf);
        assert_eq!(out, [vec![1.0; 50], vec![2.0; 100]].concat());
    }

    #[test]
    fn inserts_gap_between_utterances() {
        let buf = buffer_with(&[1.0, 2.0]);
        let mut player = Player::new(
            RATE,
            PlayerConfig {
                gap: Duration::from_millis(20),
                ..no_gap()
            },
        );
        player.replay(&buf);
        player.replay(&buf);
        let out = render_all(&mut player, &buf);
        assert_eq!(out.len(), 2 * (100 + 20));
        assert_eq!(&out[100..120], &[0.0; 20]);
    }

    #[test]
    fn nothing_to_replay() {
        let buf = SpeechBuffer::new(RATE, Duration::from_secs(1));
        let mut player = Player::new(RATE, no_gap());
        assert!(!player.replay(&buf));
        let mut block = [1.0; 8];
        let rendered = player.render(&buf, &mut block);
        assert_eq!(
            rendered,
            Rendered {
                written: 0,
                ended: true
            }
        );
        assert_eq!(block, [0.0; 8]);
        assert_eq!(player.behind(&buf), None);
    }

    #[test]
    fn follows_the_sentence_in_progress() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(60));
        buf.open(vec![1.0; 100]);
        let mut player = Player::new(RATE, no_gap());
        assert!(player.replay(&buf));

        // Caught up with the speaker: silence, but still playing.
        let mut out = render_playing(&mut player, &buf, 192);
        assert_eq!(non_silent(&out), vec![1.0; 100]);
        assert!(player.is_playing());

        buf.append(&[2.0; 100]);
        out.extend(render_playing(&mut player, &buf, 192));
        buf.append(&[3.0; 30]);
        out.extend(render_playing(&mut player, &buf, 64));
        assert_eq!(
            non_silent(&out),
            [vec![1.0; 100], vec![2.0; 100], vec![3.0; 30]].concat()
        );

        // They stop talking: playback ends.
        buf.close();
        out.extend(render_all(&mut player, &buf));
        assert_eq!(
            non_silent(&out),
            [vec![1.0; 100], vec![2.0; 100], vec![3.0; 30]].concat()
        );
        assert!(!player.is_playing());
    }

    #[test]
    fn ends_after_the_sentence_closes_plus_the_gap() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(60));
        buf.open(vec![1.0; 100]);
        let mut player = Player::new(
            RATE,
            PlayerConfig {
                gap: Duration::from_millis(20),
                ..no_gap()
            },
        );
        player.replay(&buf);
        render_playing(&mut player, &buf, 150);
        buf.close();
        let mut block = [1.0; 64];
        assert_eq!(
            player.render(&buf, &mut block),
            Rendered {
                written: 20,
                ended: true
            }
        );
        assert_eq!(block, [0.0; 64]);
    }

    #[test]
    fn waits_for_enough_new_audio_at_the_live_edge() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(60));
        buf.open(vec![1.0; 100]);
        let mut player = Player::new(
            RATE,
            PlayerConfig {
                live_margin: Duration::from_millis(50),
                ..no_gap()
            },
        );
        player.replay(&buf);
        let mut out = render_playing(&mut player, &buf, 128);
        assert_eq!(non_silent(&out), vec![1.0; 100]);

        // Less than the margin came in: keep waiting.
        buf.append(&[2.0; 30]);
        let waiting = render_playing(&mut player, &buf, 64);
        assert_eq!(waiting, [0.0; 64]);
        buf.append(&[2.0; 30]);
        out.extend(render_playing(&mut player, &buf, 64));
        assert_eq!(non_silent(&out), [vec![1.0; 100], vec![2.0; 60]].concat());
    }

    #[test]
    fn waits_while_speech_is_pending() {
        let mut buf = buffer_with(&[1.0]);
        buf.set_speech_pending(true);
        let mut player = Player::new(RATE, no_gap());
        player.replay(&buf);
        let mut out = render_playing(&mut player, &buf, 256);
        assert_eq!(non_silent(&out), vec![1.0; 100]);

        // The speech became an utterance: it plays.
        buf.open(vec![2.0; 50]);
        buf.set_speech_pending(false);
        out.extend(render_playing(&mut player, &buf, 128));
        assert_eq!(non_silent(&out), [vec![1.0; 100], vec![2.0; 50]].concat());
        buf.close();
        render_all(&mut player, &buf);

        // Pending speech that is dropped (a cough) ends the replay.
        let mut player = Player::new(RATE, no_gap());
        buf.set_speech_pending(true);
        player.replay(&buf);
        render_playing(&mut player, &buf, 256);
        buf.set_speech_pending(false);
        let mut block = [0.0; 64];
        assert_eq!(
            player.render(&buf, &mut block),
            Rendered {
                written: 0,
                ended: true
            }
        );
    }

    #[test]
    fn starts_at_the_sentence_in_progress_once_it_is_long_enough() {
        let config = PlayerConfig {
            min_open_start: Duration::from_millis(150),
            ..no_gap()
        };
        let mut buf = buffer_with(&[1.0]);
        let open = buf.open(vec![2.0; 100]);
        let mut player = Player::new(RATE, config.clone());
        assert!(player.replay(&buf));
        assert_eq!(player.current_id(), Some(open - 1));

        buf.append(&[2.0; 50]);
        let mut player = Player::new(RATE, config.clone());
        assert!(player.replay(&buf));
        assert_eq!(player.current_id(), Some(open));

        // Only a short sentence in progress: start there anyway.
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(60));
        let open = buf.open(vec![2.0; 10]);
        let mut player = Player::new(RATE, config);
        assert!(player.replay(&buf));
        assert_eq!(player.current_id(), Some(open));
    }

    #[test]
    fn steps_back_from_the_sentence_in_progress() {
        let mut buf = buffer_with(&[1.0, 2.0]);
        buf.open(vec![3.0; 2000]);
        let mut player = Player::new(RATE, no_gap());
        player.replay(&buf);
        assert_eq!(player.current_id(), Some(2));
        player.replay(&buf);
        assert_eq!(player.current_id(), Some(1));
    }

    fn sine(len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| 0.5 * (i as f32 * 2.0 * std::f32::consts::PI * 50.0 / RATE as f32).sin())
            .collect()
    }

    #[test]
    fn speed_changes_apply_mid_utterance() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(60));
        buf.push(sine(3000));
        let mut player = Player::new(RATE, no_gap());
        player.replay(&buf);
        render_playing(&mut player, &buf, 1000);

        player.set_speed(1.5);
        assert_eq!(player.speed(), 1.5);
        let rest = render_all(&mut player, &buf).len() as f32;
        // 2000 samples left take 2/3 as long, give or take a 30 ms frame.
        assert!((rest - 2000.0 / 1.5).abs() <= 30.0, "{rest}");
    }

    #[test]
    fn follows_the_sentence_in_progress_at_normal_speed() {
        let speech: Vec<f32> = sine(1000).iter().map(|s| s + 0.1).collect();
        let mut buf = SpeechBuffer::new(RATE, Duration::from_secs(60));
        buf.open(speech[..300].to_vec());
        let mut player = Player::new(RATE, PlayerConfig { speed: 1.5, ..no_gap() });
        player.replay(&buf);
        let caught_up = render_playing(&mut player, &buf, 320);
        assert!(non_silent(&caught_up).len() < 300);

        // Nothing left to catch up: what comes in plays as it was spoken.
        buf.append(&speech[300..600]);
        let following = render_playing(&mut player, &buf, 320);
        assert_eq!(&following[..300], &speech[300..600]);
        assert_eq!(player.speed(), 1.5);

        // The next sentence is behind again, and plays at the chosen speed.
        buf.open(speech.clone());
        buf.close();
        let next = render_all(&mut player, &buf);
        assert!((next.len() as f32 - 1000.0 / 1.5).abs() <= 30.0, "{}", next.len());
    }

    #[test]
    fn behind_shrinks_while_playing_and_grows_with_new_audio() {
        let mut buf = buffer_with(&[1.0]);
        buf.open(vec![2.0; 100]);
        let mut player = Player::new(
            RATE,
            PlayerConfig {
                gap: Duration::from_millis(20),
                ..no_gap()
            },
        );
        player.replay(&buf);
        assert_eq!(player.current_id(), Some(0));
        // 100 samples, the gap after them, and the 100 so far of the sentence in progress.
        assert_eq!(player.behind(&buf), Some(Duration::from_millis(220)));

        render_playing(&mut player, &buf, 64);
        assert_eq!(player.behind(&buf), Some(Duration::from_millis(156)));

        buf.append(&[2.0; 100]);
        assert_eq!(player.behind(&buf), Some(Duration::from_millis(256)));

        player.set_speed(2.0);
        let behind = player.behind(&buf).unwrap().as_millis();
        assert!((120..=135).contains(&behind), "{behind}");
    }

    #[test]
    fn keeps_playing_an_evicted_utterance_and_continues_with_the_oldest() {
        let mut buf = SpeechBuffer::new(RATE, Duration::from_millis(300));
        for v in [1.0, 2.0, 3.0] {
            buf.push(vec![v; 100]);
        }
        let mut player = Player::new(RATE, no_gap());
        for _ in 0..3 {
            player.replay(&buf);
        }
        assert_eq!(player.current_id(), Some(0));
        let mut out = render_playing(&mut player, &buf, 50);

        // 0 and 1 are evicted while 0 plays.
        buf.push(vec![4.0; 100]);
        buf.push(vec![5.0; 100]);
        assert_eq!(buf.oldest_id(), Some(2));
        out.extend(render_all(&mut player, &buf));
        assert_eq!(
            out,
            [vec![1.0; 100], vec![3.0; 100], vec![4.0; 100], vec![5.0; 100]].concat()
        );
    }
}
