use std::time::Duration;

use crate::buffer::{duration_to_samples, SpeechBuffer};
use crate::stretch::time_stretch;

pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 2.0;

#[derive(Debug, Clone)]
pub struct PlayerConfig {
    /// Silence inserted between replayed utterances so they don't run into each other.
    pub gap: Duration,
    /// Playback speed, `1.0` is normal. Clamped to [`MIN_SPEED`]..=[`MAX_SPEED`].
    pub speed: f32,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            gap: Duration::from_millis(250),
            speed: 1.0,
        }
    }
}

/// Decides what to replay and renders it, pulling utterances from a [`SpeechBuffer`].
///
/// Playback starts at some utterance and continues through every newer one, including ones recorded while replaying,
/// until it has caught up with the buffer. At that point the player stops and the listener is back to live audio.
#[derive(Debug)]
pub struct Player {
    sample_rate: u32,
    gap: usize,
    speed: f32,
    playing: Option<Playback>,
}

#[derive(Debug)]
struct Playback {
    id: u64,
    /// The current utterance, already time-stretched, followed by the gap.
    rendered: Vec<f32>,
    pos: usize,
}

impl Player {
    pub fn new(sample_rate: u32, config: PlayerConfig) -> Self {
        Self {
            sample_rate,
            gap: duration_to_samples(config.gap, sample_rate),
            speed: config.speed.clamp(MIN_SPEED, MAX_SPEED),
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

    /// Changes the speed. It applies from the next utterance on.
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
    }

    /// The replay button: starts at the latest utterance, or, if already replaying, steps back one utterance from
    /// the one playing now. Returns `false` if there was nothing to play.
    pub fn replay(&mut self, buffer: &SpeechBuffer) -> bool {
        let target = match self.current_id() {
            // Already at the oldest one: start it over.
            Some(id) => id.saturating_sub(1).max(buffer.oldest_id().unwrap_or(0)),
            None => match buffer.latest_id() {
                Some(id) => id,
                None => return false,
            },
        };
        self.load(buffer, target)
    }

    pub fn stop(&mut self) {
        self.playing = None;
    }

    /// Fills `out` with mono samples and returns how many it wrote. The rest of `out` is set to silence. Writing
    /// fewer than `out.len()` samples means playback has caught up and stopped.
    pub fn render(&mut self, buffer: &SpeechBuffer, out: &mut [f32]) -> usize {
        let mut written = 0;
        while written < out.len() {
            let Some(playback) = &mut self.playing else {
                break;
            };
            let remaining = &playback.rendered[playback.pos..];
            if remaining.is_empty() {
                let next = playback.id + 1;
                if !self.load(buffer, next) {
                    break;
                }
                continue;
            }
            let n = remaining.len().min(out.len() - written);
            out[written..written + n].copy_from_slice(&remaining[..n]);
            playback.pos += n;
            written += n;
        }
        out[written..].fill(0.0);
        written
    }

    /// Starts playing utterance `id` (or the oldest one still buffered, if `id` was evicted). Stops if it doesn't
    /// exist yet.
    fn load(&mut self, buffer: &SpeechBuffer, id: u64) -> bool {
        let id = id.max(buffer.oldest_id().unwrap_or(id));
        let Some(utterance) = buffer.get(id) else {
            self.playing = None;
            return false;
        };
        let mut rendered = time_stretch(&utterance.samples, self.speed, self.sample_rate);
        rendered.resize(rendered.len() + (self.gap as f32 / self.speed) as usize, 0.0);
        self.playing = Some(Playback { id, rendered, pos: 0 });
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
            speed: 1.0,
        }
    }

    fn render_all(player: &mut Player, buf: &SpeechBuffer) -> Vec<f32> {
        let mut out = Vec::new();
        let mut block = [0.0; 64];
        loop {
            let n = player.render(buf, &mut block);
            out.extend_from_slice(&block[..n]);
            if n < block.len() {
                return out;
            }
        }
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
                speed: 1.0,
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
        assert_eq!(player.render(&buf, &mut block), 0);
        assert_eq!(block, [0.0; 8]);
    }
}
