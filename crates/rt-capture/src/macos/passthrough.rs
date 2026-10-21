//! Ducking on macOS. macOS has no per-app volume, so a ducked app is muted (through a tap that mutes, see `tap`) and
//! we play what its capture tap records on the app's own device ourselves, at the duck level. Taps record before the
//! mute, so capture keeps the full level.
//!
//! Handing the app's audio over to our replay can't be timed to the sample: the mute lands some milliseconds after we
//! ask for it, and our replay reaches the speaker through a path of its own. The order makes sure the app is never
//! silent instead (a gap): our replay fades in to full level and is heard for [`OVERLAP`] before the app is muted, and
//! for [`OVERLAP`] after it is unmuted before it fades out. It is heard later than the app's own audio (see
//! [`IoStatus::latency`]), which the overlap allows for. While both play, the app is up to twice as loud (+6 dB) for a
//! moment. Only then does the replay fade to the duck level, over [`FADE`]. At level 0 nothing is replayed:
//! ducking only mutes, and restoring only unmutes.
//!
//! [`Sequencer`] is the control side, which `set_app_volume` drives and which mutes and unmutes the tap. [`Player`] is
//! the IOProc's side: the replay's gain, which follows the sequencer's latest [`Command`] one sample at a time, so our
//! replay itself never jumps. [`interleave`] and [`play`] move the samples in and out of Core Audio's buffers.

use std::time::Duration;

/// How long our replay fades in when it takes over from the app, and out when it hands the app back, so it doesn't
/// click.
pub(crate) const HANDOVER_FADE: Duration = Duration::from_millis(5);
/// How far from our request the OS's mute or unmute may land, either way, without a gap.
pub(crate) const OVERLAP: Duration = Duration::from_millis(20);
/// A fade the whole way between full level and silence takes this long; shorter ones take their share of it.
pub(crate) const FADE: Duration = Duration::from_millis(150);

/// What the IOProc does with the replay: fade its gain to `gain`, quickly (over [`HANDOVER_FADE`]) or not (over
/// [`FADE`]). At gain 0 nothing is replayed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Command {
    pub gain: f32,
    pub quick: bool,
}

impl Command {
    /// Nothing replayed.
    pub(crate) const SILENT: Self = Self { gain: 0.0, quick: true };

    /// The command as one word, so the IOProc reads it from one atomic.
    pub(crate) fn pack(self) -> u64 {
        u64::from(self.gain.to_bits()) | (u64::from(self.quick) << 32)
    }

    pub(crate) fn unpack(word: u64) -> Self {
        Self {
            gain: f32::from_bits(word as u32),
            quick: word & (1 << 32) != 0,
        }
    }
}

/// What the IOProc has done so far, as the control side sees it between IO cycles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct IoStatus {
    /// Frames played since the tap started.
    pub frames: u64,
    /// The replay's gain at the end of the latest IO cycle.
    pub gain: f32,
    /// How much later than the app's own audio the replay is heard, in frames: it goes through the tap, our IOProc and
    /// the device's output buffers.
    pub latency: u64,
}

/// What the control side does next to bring the app to its duck level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Step {
    /// Give the IOProc this command, then ask again once it has played more.
    Wait(Command),
    /// Mute the app (`true`) or unmute it, tell [`Sequencer::set_muted`], and ask again right away.
    Mute(bool),
    /// The level is reached. The IOProc keeps this command.
    Done(Command),
}

/// The control side of ducking: which [`Step`] brings the app from where it is to the level it should be at.
pub(crate) struct Sequencer {
    target: f32,
    muted: bool,
    /// [`IoStatus::frames`] when the app was last muted or unmuted.
    mute_changed: Option<u64>,
    /// [`IoStatus::frames`] when the replay was first seen at full level, before the app is muted.
    full_since: Option<u64>,
    /// [`OVERLAP`] in frames.
    overlap: u64,
}

impl Sequencer {
    /// Not ducked: the app plays, nothing is replayed.
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            target: 1.0,
            muted: false,
            mute_changed: None,
            full_since: None,
            overlap: frames(OVERLAP, sample_rate),
        }
    }

    /// The level to duck to: 1 is not ducked, 0 muted. A new target takes over from wherever the last one got.
    pub(crate) fn set_target(&mut self, level: f32) {
        self.target = if level.is_nan() { 1.0 } else { level.clamp(0.0, 1.0) };
    }

    /// Whether the app is muted, as far as this sequencer asked for.
    #[cfg(test)]
    fn muted(&self) -> bool {
        self.muted
    }

    /// The app was muted (`true`) or unmuted, by the latest [`Step::Mute`].
    pub(crate) fn set_muted(&mut self, muted: bool, io: IoStatus) {
        self.muted = muted;
        self.mute_changed = Some(io.frames);
    }

    /// Muting the app failed, so it plays as it did, unmuted: the replay goes out (quickly, as when it hands the app
    /// back) instead of doubling it, and the app isn't ducked any more. Returns the command for the IOProc.
    pub(crate) fn mute_failed(&mut self) -> Command {
        self.target = 1.0;
        self.muted = false;
        self.full_since = None;
        Command::SILENT
    }

    /// What to do next, given what the IOProc has done.
    pub(crate) fn next(&mut self, io: IoStatus) -> Step {
        let level = self.target;
        if level >= 1.0 {
            self.restore(io)
        } else if level <= 0.0 {
            self.mute_only(io)
        } else {
            self.replay(level, io)
        }
    }

    /// To full level: the app plays, nothing is replayed.
    fn restore(&mut self, io: IoStatus) -> Step {
        let fade_up = Command {
            gain: 1.0,
            quick: false,
        };
        if self.muted {
            // Back to full level before the app takes over again. A replay at 0 has nothing to hand over.
            if io.gain > 0.0 && io.gain < 1.0 {
                return Step::Wait(fade_up);
            }
            return Step::Mute(false);
        }
        if io.gain > 0.0 {
            // Full level until an unmute that lands late has landed, then out. The replay is heard `latency` after it
            // plays, so that much of the overlap is there anyway.
            let hold = self.overlap.saturating_sub(io.latency);
            if self.mute_changed.is_some_and(|at| io.frames < at + hold) {
                return Step::Wait(fade_up);
            }
            return Step::Wait(Command::SILENT);
        }
        Step::Done(Command::SILENT)
    }

    /// To 0: the app is muted, nothing is replayed.
    fn mute_only(&mut self, io: IoStatus) -> Step {
        if !self.muted {
            return Step::Mute(true);
        }
        let fade_out = Command {
            gain: 0.0,
            quick: false,
        };
        if io.gain > 0.0 {
            return Step::Wait(fade_out);
        }
        Step::Done(fade_out)
    }

    /// To `level` between 0 and 1: the app muted, its audio replayed at `level`.
    fn replay(&mut self, level: f32, io: IoStatus) -> Step {
        let full = Command { gain: 1.0, quick: true };
        if !self.muted {
            // Take over: the replay at full level for a while before the app is muted, so a mute that lands early
            // leaves no gap. The replay is heard `latency` after it plays, so it plays that much longer.
            if io.gain < 1.0 {
                self.full_since = None;
                return Step::Wait(full);
            }
            let since = *self.full_since.get_or_insert(io.frames);
            if io.frames < since + self.overlap + io.latency {
                return Step::Wait(full);
            }
            return Step::Mute(true);
        }
        // Just muted: full level until a mute that lands late has landed. A replay that isn't at full level (it fades
        // between levels) has no mute to wait for.
        if io.gain >= 1.0 && !self.overlapped(io) {
            return Step::Wait(full);
        }
        let fade = Command {
            gain: level,
            quick: false,
        };
        if io.gain == level {
            Step::Done(fade)
        } else {
            Step::Wait(fade)
        }
    }

    /// Whether the latest mute or unmute is [`OVERLAP`] old.
    fn overlapped(&self, io: IoStatus) -> bool {
        self.mute_changed.is_none_or(|at| io.frames >= at + self.overlap)
    }
}

/// The replay's gain in the IOProc. It follows a [`Command`] one sample at a time.
pub(crate) struct Player {
    gain: f32,
    /// How much the gain moves per frame, for a [`FADE`] and for a [`HANDOVER_FADE`].
    step: f32,
    quick_step: f32,
}

impl Player {
    /// Silent.
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            gain: 0.0,
            step: 1.0 / frames(FADE, sample_rate) as f32,
            quick_step: 1.0 / frames(HANDOVER_FADE, sample_rate) as f32,
        }
    }

    pub(crate) fn gain(&self) -> f32 {
        self.gain
    }

    /// The gain for the next frame, one step closer to what `command` asks for.
    pub(crate) fn next(&mut self, command: Command) -> f32 {
        let target = command.gain.clamp(0.0, 1.0);
        let step = if command.quick { self.quick_step } else { self.step };
        self.gain = if self.gain < target {
            (self.gain + step).min(target)
        } else {
            (self.gain - step).max(target)
        };
        self.gain
    }
}

/// `duration` in frames at `sample_rate`, at least 1.
fn frames(duration: Duration, sample_rate: u32) -> u64 {
    ((duration.as_secs_f64() * f64::from(sample_rate)).round() as u64).max(1)
}

/// One buffer of a Core Audio buffer list: `channels` interleaved channels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Buffer<'a> {
    pub channels: usize,
    pub samples: &'a [f32],
}

/// One output buffer of a Core Audio buffer list.
#[derive(Debug)]
pub(crate) struct BufferMut<'a> {
    pub channels: usize,
    pub samples: &'a mut [f32],
}

/// Interleaves the last `channels` channels of `buffers` into `out`, and returns how many frames that is. The tap is
/// the last input of its aggregate device; any input before it belongs to the app's device (a headset's microphone),
/// which isn't ours to record. A stream that isn't interleaved comes as one buffer per channel. 0 if `buffers` has
/// fewer channels than that.
pub(crate) fn interleave(buffers: &[Buffer], channels: usize, out: &mut Vec<f32>) -> usize {
    out.clear();
    // (buffer, channel in it) of each channel to take, last first.
    let mut picked = [(0usize, 0usize); MAX_CHANNELS];
    let mut count = 0;
    'buffers: for (b, buffer) in buffers.iter().enumerate().rev() {
        for c in (0..buffer.channels).rev() {
            if count == channels.min(MAX_CHANNELS) {
                break 'buffers;
            }
            picked[count] = (b, c);
            count += 1;
        }
    }
    if channels == 0 || count < channels {
        return 0;
    }
    let picked = &mut picked[..count];
    picked.reverse();
    let frames = picked
        .iter()
        .map(|&(b, _)| buffers[b].samples.len() / buffers[b].channels)
        .min()
        .unwrap_or(0);
    for frame in 0..frames {
        for &(b, c) in picked.iter() {
            let buffer = &buffers[b];
            out.push(buffer.samples[frame * buffer.channels + c]);
        }
    }
    frames
}

/// The most channels [`interleave`] takes; a tap is stereo.
const MAX_CHANNELS: usize = 8;

/// Plays interleaved `input` of `channels` channels on the output buffers `out`, each frame at the gain `player` gives
/// it for `command`. Output channels beyond the input's stay silent; a mono output gets the average. Output frames the
/// input doesn't have are silent too. Without output buffers the player still moves on by the input's frames, so a
/// fade finishes even when nothing can be heard. Returns how many frames the player moved on.
pub(crate) fn play(
    input: &[f32],
    channels: usize,
    player: &mut Player,
    command: Command,
    out: &mut [BufferMut],
) -> usize {
    let in_frames = input.len().checked_div(channels).unwrap_or(0);
    let out_channels: usize = out.iter().map(|b| b.channels).sum();
    let out_frames = out
        .iter()
        .filter(|b| b.channels > 0)
        .map(|b| b.samples.len() / b.channels)
        .max();
    let Some(out_frames) = out_frames.filter(|_| out_channels > 0) else {
        for _ in 0..in_frames {
            player.next(command);
        }
        return in_frames;
    };
    for frame in 0..out_frames {
        let gain = player.next(command);
        let source = input
            .get(frame * channels..(frame + 1) * channels)
            .filter(|_| channels > 0);
        let mut channel = 0;
        for buffer in out.iter_mut() {
            for c in 0..buffer.channels {
                let sample = match source {
                    Some(source) if out_channels == 1 => source.iter().sum::<f32>() / channels as f32,
                    Some(source) => source.get(channel).copied().unwrap_or(0.0),
                    None => 0.0,
                };
                if let Some(slot) = buffer.samples.get_mut(frame * buffer.channels + c) {
                    *slot = sample * gain;
                }
                channel += 1;
            }
        }
    }
    out_frames
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    /// A common IO buffer size.
    const CYCLE: u64 = 512;

    fn ms(frames: u64) -> f64 {
        frames as f64 * 1000.0 / f64::from(RATE)
    }

    fn to_frames(duration: Duration) -> u64 {
        frames(duration, RATE)
    }

    /// The IOProc and the control side, simulated: the IOProc plays cycles of [`CYCLE`] frames, and the control side
    /// looks between them.
    struct Sim {
        sequencer: Sequencer,
        player: Player,
        command: Command,
        frames: u64,
        /// How much later than the app's own audio the replay is heard, in frames.
        latency: u64,
        /// The replay's gain of each frame played.
        gains: Vec<f32>,
        /// The frame at which the app was muted (`true`) or unmuted, as asked.
        mutes: Vec<(u64, bool)>,
        /// Muting the app fails, as making the muting tap may.
        mute_fails: bool,
    }

    impl Sim {
        fn new() -> Self {
            Self {
                sequencer: Sequencer::new(RATE),
                player: Player::new(RATE),
                command: Command::SILENT,
                frames: 0,
                latency: 0,
                gains: Vec::new(),
                mutes: Vec::new(),
                mute_fails: false,
            }
        }

        /// A replay that is heard `latency_ms` after the app's own audio.
        fn with_latency(latency_ms: u64) -> Self {
            Self {
                latency: latency_ms * u64::from(RATE) / 1000,
                ..Self::new()
            }
        }

        fn io(&self) -> IoStatus {
            IoStatus {
                frames: self.frames,
                gain: self.player.gain(),
                latency: self.latency,
            }
        }

        /// Asks the sequencer what to do and does it, like `Tap::duck`. `true` once it's done, or failed.
        fn control(&mut self) -> bool {
            loop {
                match self.sequencer.next(self.io()) {
                    Step::Mute(true) if self.mute_fails => {
                        self.command = self.sequencer.mute_failed();
                        return true;
                    }
                    Step::Mute(muted) => {
                        self.mutes.push((self.frames, muted));
                        self.sequencer.set_muted(muted, self.io());
                    }
                    Step::Wait(command) => {
                        self.command = command;
                        return false;
                    }
                    Step::Done(command) => {
                        self.command = command;
                        return true;
                    }
                }
            }
        }

        fn play_cycle(&mut self) {
            for _ in 0..CYCLE {
                let gain = self.player.next(self.command);
                self.gains.push(gain);
            }
            self.frames += CYCLE;
        }

        /// Ducks to `level` and plays until the sequencer is done, like `set_app_volume`. Returns the frame it started
        /// at.
        fn duck(&mut self, level: f32) -> u64 {
            let start = self.frames;
            self.sequencer.set_target(level);
            for _ in 0..1000 {
                if self.control() {
                    return start;
                }
                self.play_cycle();
            }
            panic!("ducking to {level} never finished");
        }

        /// Ducks to `level` until `stop` says so, without finishing.
        fn duck_until(&mut self, level: f32, stop: impl Fn(&Self) -> bool) {
            self.sequencer.set_target(level);
            for _ in 0..1000 {
                if self.control() || stop(self) {
                    return;
                }
                self.play_cycle();
            }
            panic!("never got there");
        }

        /// Plays `cycles` more with the latest command.
        fn settle(&mut self, cycles: u64) {
            for _ in 0..cycles {
                self.play_cycle();
            }
        }
    }

    /// The largest step between two neighbouring gains.
    fn steepest(gains: &[f32]) -> f32 {
        gains.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn commands_survive_packing() {
        for command in [
            Command::SILENT,
            Command {
                gain: 0.2,
                quick: false,
            },
            Command { gain: 1.0, quick: true },
        ] {
            assert_eq!(Command::unpack(command.pack()), command);
        }
    }

    #[test]
    fn engaging_replays_at_full_level_then_mutes_then_fades_to_the_level() {
        let mut sim = Sim::new();
        sim.duck(0.2);
        let gains = &sim.gains;

        // One mute, once the replay has played at full level for the overlap.
        assert_eq!(sim.mutes.len(), 1, "{:?}", sim.mutes);
        let (muted_at, muted) = sim.mutes[0];
        assert!(muted);
        let full_at = gains
            .iter()
            .position(|&g| g == 1.0)
            .expect("the replay reaches full level") as u64;
        assert!(ms(full_at) <= 5.0 + 0.1, "full level after {} ms", ms(full_at));
        assert!(
            muted_at >= full_at + to_frames(OVERLAP),
            "muted {} ms after full level",
            ms(muted_at - full_at)
        );

        // Full level until the overlap after the mute has passed, then a fade to the level.
        let fade_at = gains.iter().rposition(|&g| g == 1.0).unwrap() as u64 + 1;
        assert!(fade_at >= muted_at + to_frames(OVERLAP));
        assert!(gains[full_at as usize..fade_at as usize].iter().all(|&g| g == 1.0));
        let fade = &gains[fade_at as usize..];
        assert!(fade.windows(2).all(|w| w[1] <= w[0]), "the fade only goes down");
        assert_eq!(*gains.last().unwrap(), 0.2);
        let fade_ms = ms(fade.iter().position(|&g| g == 0.2).unwrap() as u64 + 1);
        assert!((fade_ms - 0.8 * 150.0).abs() < 1.0, "a fade of 80% takes {fade_ms} ms");
        assert!(sim.sequencer.muted());
    }

    #[test]
    fn restoring_ends_unmuted_and_silent() {
        let mut sim = Sim::new();
        sim.duck(0.2);
        sim.settle(10);
        let start = sim.duck(1.0) as usize;
        let gains = &sim.gains[start..];

        let (unmuted_at, muted) = *sim.mutes.last().unwrap();
        assert!(!muted);
        assert!(!sim.sequencer.muted());
        // Full level before the unmute, and for the overlap after it.
        let full_at = start as u64 + gains.iter().position(|&g| g == 1.0).unwrap() as u64;
        assert!(unmuted_at >= full_at);
        let out_at = start as u64 + gains.iter().rposition(|&g| g == 1.0).unwrap() as u64 + 1;
        assert!(out_at >= unmuted_at + to_frames(OVERLAP));
        // Then out, quickly but without a jump, and nothing is replayed any more.
        assert_eq!(*gains.last().unwrap(), 0.0);
        assert_eq!(sim.command, Command::SILENT);
        assert!(steepest(gains) <= 1.0 / to_frames(HANDOVER_FADE) as f32 + 1e-6);
        sim.settle(5);
        assert!(
            sim.gains[sim.gains.len() - 5 * CYCLE as usize..]
                .iter()
                .all(|&g| g == 0.0)
        );
    }

    #[test]
    fn zero_only_mutes_and_restoring_from_it_only_unmutes() {
        let mut sim = Sim::new();
        sim.duck(0.0);
        assert_eq!(sim.mutes, [(0, true)]);
        sim.settle(10);
        sim.duck(1.0);
        assert_eq!(sim.mutes.iter().map(|m| m.1).collect::<Vec<_>>(), [true, false]);
        assert!(sim.gains.iter().all(|&g| g == 0.0), "nothing is replayed");
    }

    #[test]
    fn from_zero_to_a_level_and_back_while_ducked_fades_the_replay_only() {
        let mut sim = Sim::new();
        sim.duck(0.0);
        let start = sim.duck(0.2) as usize;
        assert_eq!(sim.mutes, [(0, true)], "the app stays muted");
        let up = &sim.gains[start..];
        assert!(up.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(up[0], 1.0 / to_frames(FADE) as f32, "the replay fades in from silence");
        assert_eq!(*up.last().unwrap(), 0.2);

        let start = sim.duck(0.0) as usize;
        let down = &sim.gains[start..];
        assert!(down.windows(2).all(|w| w[1] <= w[0]));
        assert_eq!(*down.last().unwrap(), 0.0);
        assert_eq!(sim.mutes, [(0, true)]);
        assert!(steepest(&sim.gains) <= 1.0 / to_frames(FADE) as f32 + 1e-6);
    }

    #[test]
    fn a_new_level_during_a_fade_takes_over_from_where_it_got() {
        let mut sim = Sim::new();
        sim.duck_until(0.2, |sim| sim.sequencer.muted() && sim.player.gain() < 0.7);
        let turned_at = sim.gains.len();
        let turned_from = sim.player.gain();
        assert!(turned_from < 0.7 && turned_from > 0.2);
        sim.duck(0.8);
        let after = &sim.gains[turned_at..];
        assert!(
            after.windows(2).all(|w| w[1] >= w[0]),
            "it turns around without first reaching 0.2"
        );
        assert_eq!(*sim.gains.last().unwrap(), 0.8);
        assert_eq!(sim.mutes.len(), 1);
        assert!(steepest(&sim.gains) <= 1.0 / to_frames(HANDOVER_FADE) as f32 + 1e-6);
    }

    #[test]
    fn a_mute_that_fails_takes_the_replay_out_instead_of_doubling_the_app() {
        let mut sim = Sim::new();
        sim.mute_fails = true;
        sim.duck(0.2);
        assert!(sim.mutes.is_empty());
        assert!(!sim.sequencer.muted());
        // The replay had reached full level for the takeover: out again, quickly but without a jump.
        assert_eq!(sim.command, Command::SILENT);
        sim.settle(2);
        assert_eq!(*sim.gains.last().unwrap(), 0.0);
        assert!(steepest(&sim.gains) <= 1.0 / to_frames(HANDOVER_FADE) as f32 + 1e-6);

        // The app isn't ducked, so restoring has nothing to do.
        let start = sim.duck(1.0);
        assert_eq!(sim.frames, start, "restored at once");
        assert!(sim.mutes.is_empty());

        // The next duck starts over.
        sim.mute_fails = false;
        sim.duck(0.2);
        assert_eq!(sim.mutes.iter().map(|m| m.1).collect::<Vec<_>>(), [true]);
        assert_eq!(*sim.gains.last().unwrap(), 0.2);
    }

    #[test]
    fn restoring_before_the_app_is_muted_never_mutes_it() {
        let mut sim = Sim::new();
        sim.duck_until(0.2, |sim| sim.player.gain() == 1.0);
        assert!(sim.mutes.is_empty());
        sim.duck(1.0);
        assert!(sim.mutes.is_empty(), "{:?}", sim.mutes);
        assert_eq!(*sim.gains.last().unwrap(), 0.0);
    }

    /// The level of the app as heard: its own audio until the OS mutes it, plus our replay.
    struct Heard {
        /// The app's own path: 1 or 0.
        app: Vec<f32>,
        /// Both together, as a gain on the app's audio: the replay carries the same audio, `latency` frames later.
        sum: Vec<f32>,
    }

    /// The level heard over `gains` (from frame `start` of the simulation) when the mute or unmute asked at
    /// `asked_at` lands `offset_ms` later (earlier if negative), with the app `muted_before` it, and the replay heard
    /// `latency` frames after it was played (silent until then, or at the duck level if the app was muted before).
    fn heard(gains: &[f32], start: u64, asked_at: u64, offset_ms: i64, muted_before: bool, latency: u64) -> Heard {
        let lands = asked_at as i64 - start as i64 + offset_ms * i64::from(RATE) / 1000;
        let app: Vec<f32> = (0..gains.len() as i64)
            .map(|n| if (n < lands) != muted_before { 1.0 } else { 0.0 })
            .collect();
        let latency = latency as usize;
        // Before the window the replay was silent, or at the duck level.
        let before = if muted_before { 0.2 } else { 0.0 };
        let replay = |n: usize| if n >= latency { gains[n - latency] } else { before };
        let sum = app.iter().enumerate().map(|(n, a)| a + replay(n)).collect();
        Heard { app, sum }
    }

    /// A 1 kHz tone at half of full scale.
    fn tone(n: usize) -> f32 {
        0.5 * (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / RATE as f32).sin()
    }

    /// The largest step between neighbouring samples of the 1 kHz tone at full level, plus what the quickest fade adds.
    fn max_tone_step() -> f32 {
        0.5 * 2.0 * std::f32::consts::PI * 1000.0 / RATE as f32 + 0.5 / to_frames(HANDOVER_FADE) as f32 + 1e-4
    }

    /// The click check: the replay itself never jumps, and the sum jumps only where the OS mutes or unmutes the app. The
    /// sum is only checked for a replay in step with the app: a later one is a copy of the tone out of phase.
    fn assert_no_clicks(gains: &[f32], heard: &Heard, what: &str, in_step: bool) {
        let replay: Vec<f32> = gains.iter().enumerate().map(|(n, g)| tone(n) * g).collect();
        let worst = replay.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst <= max_tone_step(), "{what}: the replay jumps by {worst}");
        for n in (1..gains.len()).filter(|_| in_step) {
            if heard.app[n] != heard.app[n - 1] {
                continue;
            }
            let step = (tone(n) * heard.sum[n] - tone(n - 1) * heard.sum[n - 1]).abs();
            assert!(
                step <= 2.0 * max_tone_step(),
                "{what}: the sum jumps by {step} at {} ms",
                ms(n as u64)
            );
        }
    }

    /// The longest stretch over which the sum is louder than the app alone, in ms, and its peak in dB.
    fn bump(heard: &Heard) -> (f64, f64) {
        let (mut longest, mut run) = (0u64, 0u64);
        for &level in &heard.sum {
            run = if level > 1.0 + 1e-3 { run + 1 } else { 0 };
            longest = longest.max(run);
        }
        let peak = heard.sum.iter().copied().fold(0.0, f32::max);
        (ms(longest), 20.0 * f64::from(peak).log10())
    }

    /// Offsets of the OS's mute from our request, in ms: early (negative) when our replay reaches the speaker later
    /// than the app does, late when the mute takes its time.
    const OFFSETS: [i64; 9] = [-20, -15, -10, -5, 0, 5, 10, 15, 20];

    /// How much later than the app's own audio the replay may be heard, in ms: Core Audio's buffers, and the aggregate
    /// device's resampling of the tap.
    const LATENCIES: [u64; 3] = [0, 30, 60];

    #[test]
    fn taking_over_leaves_no_gap_and_no_click_wherever_the_mute_lands() {
        for latency in LATENCIES {
            let mut sim = Sim::with_latency(latency);
            sim.duck(0.2);
            sim.settle(4 + 2 * latency / ms(CYCLE) as u64);
            let gains = &sim.gains;
            let (asked_at, _) = sim.mutes[0];
            // The replay as heard starts fading here.
            let fade_at = gains.iter().rposition(|&g| g == 1.0).unwrap() + 1 + sim.latency as usize;
            for offset in OFFSETS {
                let what = format!("replay {latency} ms late, mute {offset:+} ms");
                let heard = heard(gains, 0, asked_at, offset, false, sim.latency);
                // Never quieter than the app was until the fade starts, and never below the level.
                for (n, &level) in heard.sum.iter().enumerate() {
                    let least = if n < fade_at { 1.0 } else { 0.2 };
                    assert!(level >= least - 1e-6, "{what}: a gap at {} ms ({level})", ms(n as u64));
                }
                let (length, peak) = bump(&heard);
                assert!(peak <= 6.03, "{what}: a bump of {peak:.2} dB");
                let most = 5.0 + 20.0 + 2.0 * ms(CYCLE) + offset.max(0) as f64;
                assert!(length <= most, "{what}: a bump of {length:.1} ms, more than {most:.1}");
                assert_no_clicks(gains, &heard, &what, latency == 0);
            }
        }
    }

    #[test]
    fn handing_back_leaves_no_gap_and_no_click_wherever_the_unmute_lands() {
        for latency in LATENCIES {
            let mut sim = Sim::with_latency(latency);
            sim.duck(0.2);
            sim.settle(10);
            let start = sim.duck(1.0);
            sim.settle(4 + 2 * latency / ms(CYCLE) as u64);
            let gains = &sim.gains[start as usize..];
            let (asked_at, _) = *sim.mutes.last().unwrap();
            let full_at = gains.iter().position(|&g| g == 1.0).unwrap() + sim.latency as usize;
            for offset in OFFSETS {
                let what = format!("replay {latency} ms late, unmute {offset:+} ms");
                let heard = heard(gains, start, asked_at, offset, true, sim.latency);
                // Never below the level, and never quieter than full level once the replay is back there.
                for (n, &level) in heard.sum.iter().enumerate() {
                    let least = if n >= full_at { 1.0 } else { 0.2 };
                    assert!(level >= least - 1e-6, "{what}: a gap at {} ms ({level})", ms(n as u64));
                }
                assert_eq!(*heard.sum.last().unwrap(), 1.0, "{what}: the app alone at the end");
                let (length, peak) = bump(&heard);
                assert!(peak <= 6.03, "{what}: a bump of {peak:.2} dB");
                let most = 5.0 + 20f64.max(latency as f64) + 2.0 * ms(CYCLE) + (-offset).max(0) as f64;
                assert!(length <= most, "{what}: a bump of {length:.1} ms, more than {most:.1}");
                assert_no_clicks(gains, &heard, &what, latency == 0);
            }
        }
    }

    #[test]
    fn a_late_replay_is_heard_before_the_app_is_muted() {
        let mut sim = Sim::with_latency(60);
        sim.duck(0.2);
        let full_at = sim.gains.iter().position(|&g| g == 1.0).unwrap() as u64;
        let (muted_at, _) = sim.mutes[0];
        assert!(muted_at >= full_at + to_frames(OVERLAP) + sim.latency);
        // Handing back, the replay is heard that much longer anyway, so it stops sooner.
        sim.settle(10);
        let start = sim.duck(1.0) as usize;
        let (unmuted_at, _) = *sim.mutes.last().unwrap();
        let out_at = start as u64 + sim.gains[start..].iter().rposition(|&g| g == 1.0).unwrap() as u64 + 1;
        assert!(
            out_at - unmuted_at < to_frames(OVERLAP),
            "{} ms",
            ms(out_at - unmuted_at)
        );
    }

    #[test]
    fn a_player_moves_one_step_per_frame() {
        let mut player = Player::new(RATE);
        let quick = Command { gain: 1.0, quick: true };
        let first = player.next(quick);
        assert_eq!(first, 1.0 / to_frames(HANDOVER_FADE) as f32);
        for _ in 0..to_frames(HANDOVER_FADE) {
            player.next(quick);
        }
        assert_eq!(player.gain(), 1.0);
        let slow = Command {
            gain: 0.5,
            quick: false,
        };
        let frames = (0..).take_while(|_| player.next(slow) > 0.5).count() as u64 + 1;
        assert!(
            frames.abs_diff(to_frames(FADE) / 2) <= 1,
            "half a fade took {frames} frames"
        );
        assert_eq!(player.gain(), 0.5);
        // Out of range asks for the nearest valid gain.
        for _ in 0..to_frames(FADE) {
            player.next(Command {
                gain: 7.0,
                quick: false,
            });
        }
        assert_eq!(player.gain(), 1.0);
    }

    #[test]
    fn a_stream_that_is_not_interleaved_comes_out_interleaved() {
        let left = [1.0, 2.0, 3.0];
        let right = [-1.0, -2.0, -3.0];
        let buffers = [
            Buffer {
                channels: 1,
                samples: &left,
            },
            Buffer {
                channels: 1,
                samples: &right,
            },
        ];
        let mut out = Vec::new();
        assert_eq!(interleave(&buffers, 2, &mut out), 3);
        assert_eq!(out, [1.0, -1.0, 2.0, -2.0, 3.0, -3.0]);
    }

    #[test]
    fn an_interleaved_stream_is_copied() {
        let tap = [1.0, -1.0, 2.0, -2.0];
        let mut out = vec![9.0; 10];
        let buffers = [Buffer {
            channels: 2,
            samples: &tap,
        }];
        assert_eq!(interleave(&buffers, 2, &mut out), 2);
        assert_eq!(out, tap);
    }

    #[test]
    fn the_input_of_the_apps_device_before_the_tap_is_left_out() {
        // A headset's microphone (stereo, then mono) comes before the tap's stream.
        let mic = [0.5; 6];
        let mono_mic = [0.25; 3];
        let tap_left = [1.0, 2.0, 3.0];
        let tap_right = [-1.0, -2.0, -3.0];
        let interleaved_tap = [1.0, -1.0, 2.0, -2.0, 3.0, -3.0];
        let mut out = Vec::new();
        let buffers = [
            Buffer {
                channels: 2,
                samples: &mic,
            },
            Buffer {
                channels: 2,
                samples: &interleaved_tap,
            },
        ];
        assert_eq!(interleave(&buffers, 2, &mut out), 3);
        assert_eq!(out, interleaved_tap);
        let buffers = [
            Buffer {
                channels: 1,
                samples: &mono_mic,
            },
            Buffer {
                channels: 1,
                samples: &tap_left,
            },
            Buffer {
                channels: 1,
                samples: &tap_right,
            },
        ];
        assert_eq!(interleave(&buffers, 2, &mut out), 3);
        assert_eq!(out, interleaved_tap);
    }

    #[test]
    fn too_few_channels_give_nothing() {
        let mono = [1.0, 2.0];
        let mut out = vec![5.0];
        let buffers = [Buffer {
            channels: 1,
            samples: &mono,
        }];
        assert_eq!(interleave(&buffers, 2, &mut out), 0);
        assert!(out.is_empty());
        assert_eq!(interleave(&[], 2, &mut out), 0);
        assert_eq!(interleave(&buffers, 0, &mut out), 0);
    }

    /// A player already at `gain`.
    fn player_at(gain: f32) -> (Player, Command) {
        let mut player = Player::new(RATE);
        let command = Command { gain, quick: true };
        for _ in 0..to_frames(HANDOVER_FADE) {
            player.next(command);
        }
        (player, command)
    }

    #[test]
    fn the_replay_plays_on_the_outputs_channels() {
        let input = [1.0, -1.0, 0.5, -0.5];
        let (mut player, command) = player_at(0.5);

        let mut stereo = [9.0; 4];
        play(
            &input,
            2,
            &mut player,
            command,
            &mut [BufferMut {
                channels: 2,
                samples: &mut stereo,
            }],
        );
        assert_eq!(stereo, [0.5, -0.5, 0.25, -0.25]);

        // Four channels: the extra two stay silent.
        let mut quad = [9.0; 8];
        play(
            &input,
            2,
            &mut player,
            command,
            &mut [BufferMut {
                channels: 4,
                samples: &mut quad,
            }],
        );
        assert_eq!(quad, [0.5, -0.5, 0.0, 0.0, 0.25, -0.25, 0.0, 0.0]);

        // Not interleaved: one buffer per channel.
        let (mut left, mut right) = ([9.0; 2], [9.0; 2]);
        play(
            &input,
            2,
            &mut player,
            command,
            &mut [
                BufferMut {
                    channels: 1,
                    samples: &mut left,
                },
                BufferMut {
                    channels: 1,
                    samples: &mut right,
                },
            ],
        );
        assert_eq!((left, right), ([0.5, 0.25], [-0.5, -0.25]));

        // Mono: the average.
        let mut mono = [9.0; 2];
        play(
            &[1.0, 0.0, 0.5, 0.5],
            2,
            &mut player,
            command,
            &mut [BufferMut {
                channels: 1,
                samples: &mut mono,
            }],
        );
        assert_eq!(mono, [0.25, 0.25]);
    }

    #[test]
    fn output_the_input_lacks_is_silent_and_the_gain_moves_per_frame() {
        let mut player = Player::new(RATE);
        let command = Command { gain: 1.0, quick: true };
        let mut out = [9.0; 6];
        let played = play(
            &[1.0, 1.0],
            2,
            &mut player,
            command,
            &mut [BufferMut {
                channels: 2,
                samples: &mut out,
            }],
        );
        let step = 1.0 / to_frames(HANDOVER_FADE) as f32;
        assert_eq!(played, 3);
        assert_eq!(out, [step, step, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(player.gain(), 3.0 * step);

        // Nowhere to play: the fade still moves on with the input.
        assert_eq!(play(&[1.0; 8], 2, &mut player, command, &mut []), 4);
        assert!((player.gain() - 7.0 * step).abs() < 1e-6, "{}", player.gain());
    }
}
