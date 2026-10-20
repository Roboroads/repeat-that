//! Ducking fades an app's volume in small steps instead of changing it at once.
//!
//! A capture compensates each step for audio played at that step's volume, but it can only estimate when a volume
//! change reaches the audio: Windows doesn't say, and on CI the steps of a fade came up to 10 ms early or 8 ms late,
//! by a different amount in every fade, and the odd step another 10 ms off. With one big step that leaves milliseconds
//! at 4× or a quarter of the level, an audible click. With half-dB steps 10 ms apart, the capture is a step or two
//! off for a few milliseconds per step, rarely three (1.5 dB), which nobody hears. The fade also sounds smoother.
//!
//! That only holds while the steps stay apart. A fade that falls behind, because its thread didn't get to run, goes
//! on from where it is instead of catching up: overdue steps made at once would reach the audio as one big step.

// Only the Windows backend uses this so far; the tests run everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::thread;
use std::time::{Duration, Instant};

use crate::before_duck::{BeforeDuck, Session};
use crate::clamp_duck_level;

/// The largest change in one step, in dB. Small enough that being two steps off is still inaudible.
const MAX_STEP_DB: f64 = 0.5;

/// Time between steps: one period of the Windows audio engine, so each step reaches the audio on its own. A duck to
/// 25% (−12 dB) takes 25 steps, 240 ms; one to the 5% minimum 53 steps, 520 ms.
pub(crate) const STEP_INTERVAL: Duration = Duration::from_millis(10);

/// The duck levels a ramp from `from` to `to` sets, in order: equal steps in dB of at most [`MAX_STEP_DB`], the last
/// one exactly `to`. Always at least one step, so a ramp to the current level still applies it, e.g. to sessions the
/// app started since.
pub(crate) fn steps(from: f32, to: f32) -> Vec<f32> {
    let (from, to) = (clamp_duck_level(from), clamp_duck_level(to));
    let ratio = f64::from(to) / f64::from(from);
    let db = 20.0 * ratio.log10();
    let count = ((db.abs() / MAX_STEP_DB).ceil() as usize).max(1);
    (1..=count)
        .map(|i| {
            if i == count {
                to
            } else {
                (f64::from(from) * ratio.powf(i as f64 / count as f64)) as f32
            }
        })
        .collect()
}

/// Ramps from duck level `from` to `to`: calls `step(level, duck_new)` for each of [`steps`], one every `interval`,
/// and returns after the last one. See [`next_due`] for a ramp that falls behind. `duck_new` says whether sessions
/// that aren't ducked yet join at this step: from the first step of a ramp down, but only at the last step of a ramp
/// up, so they never dip and come back up.
///
/// `last_step` is when the latest step of any ramp started. The first step keeps an interval's distance from it too,
/// so a ramp right after another doesn't make two steps at once; it is updated with every step.
pub(crate) fn run(
    from: f32,
    to: f32,
    interval: Duration,
    last_step: &mut Option<Instant>,
    mut step: impl FnMut(f32, bool),
) {
    let levels = steps(from, to);
    let down = clamp_duck_level(to) < clamp_duck_level(from);
    let mut due = last_step.map_or_else(Instant::now, |last| last + interval);
    for (i, &level) in levels.iter().enumerate() {
        thread::sleep(due.saturating_duration_since(Instant::now()));
        let started = Instant::now();
        step(level, down || i + 1 == levels.len());
        *last_step = Some(started);
        due = next_due(due, started, interval);
    }
}

/// When the step after one that was due at `due` and started at `started` is due. Normally one `interval` later, so
/// neither a sleep that overshoots a little nor the time a step takes stretches the ramp. A step that started more
/// than half an interval late, because the thread didn't get to run, moves the schedule instead: catching up would
/// make overdue steps at once, which reach the audio as one big step. So steps are always at least half an interval
/// apart.
fn next_due(due: Instant, started: Instant, interval: Duration) -> Instant {
    if started > due + interval / 2 {
        started + interval
    } else {
        due + interval
    }
}

/// One step of a ramp: gives each of `sessions` duck `level`. Sessions that aren't ducked yet are left alone unless
/// `duck_new`.
pub(crate) fn apply(sessions: &[impl Session], level: f32, duck_new: bool, before_duck: &mut BeforeDuck) {
    for session in sessions {
        if duck_new || before_duck.contains(session.id()) {
            before_duck.apply(session, level);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::MIN_DUCK_LEVEL;
    use crate::duck_journal::JournalEntry;

    fn db(level: f32) -> f64 {
        20.0 * f64::from(level).log10()
    }

    #[test]
    fn steps_are_monotonic_small_and_end_exactly_at_the_target() {
        let levels = [MIN_DUCK_LEVEL, 0.1, 0.2, 0.25, 0.5, 0.7, 0.9, 0.95, 1.0];
        for from in levels {
            for to in levels {
                let steps = steps(from, to);
                assert_eq!(steps.last(), Some(&to), "{from} → {to}: {steps:?}");
                let path: Vec<f32> = std::iter::once(from).chain(steps.iter().copied()).collect();
                for pair in path.windows(2) {
                    let change = db(pair[1]) - db(pair[0]);
                    assert!(
                        change.abs() <= MAX_STEP_DB + 1e-6,
                        "{from} → {to}: step of {change:.2} dB in {steps:?}"
                    );
                    if to < from {
                        assert!(pair[1] < pair[0], "{from} → {to}: {steps:?}");
                    } else if to > from {
                        assert!(pair[1] > pair[0], "{from} → {to}: {steps:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_duck_to_a_quarter_takes_a_quarter_second() {
        assert_eq!(steps(1.0, 0.25).len(), 25);
        assert_eq!(steps(0.25, 1.0).len(), 25);
        // The deepest duck needs more steps to keep each one small.
        assert_eq!(steps(1.0, MIN_DUCK_LEVEL).len(), 53);
    }

    #[test]
    fn a_ramp_to_the_current_level_is_one_step() {
        assert_eq!(steps(0.25, 0.25), [0.25]);
        assert_eq!(steps(1.0, 1.0), [1.0]);
        assert_eq!(steps(1.0, f32::NAN), [1.0]);
        assert_eq!(steps(1.0, 0.0), steps(1.0, MIN_DUCK_LEVEL));
    }

    #[test]
    fn new_sessions_join_a_ramp_down_at_once_and_a_ramp_up_at_the_end() {
        let joins = |from, to| {
            let mut joins = Vec::new();
            run(from, to, Duration::ZERO, &mut None, |_, duck_new| joins.push(duck_new));
            joins
        };
        assert!(joins(1.0, 0.25).iter().all(|&j| j));
        let up = joins(0.25, 0.5);
        assert_eq!(up.last(), Some(&true));
        assert!(up[..up.len() - 1].iter().all(|&j| !j), "{up:?}");
        assert_eq!(joins(0.25, 0.25), [true]);
    }

    #[test]
    fn steps_are_paced() {
        let start = Instant::now();
        let mut count = 0;
        run(1.0, 0.25, Duration::from_millis(2), &mut None, |_, _| count += 1);
        assert_eq!(count, 25);
        assert!(start.elapsed() >= Duration::from_millis(48));
    }

    #[test]
    fn steps_stay_on_their_grid() {
        let interval = Duration::from_millis(10);
        let start = Instant::now();
        let ms = |ms| Duration::from_millis(ms);
        // Each step is due one interval after the one before, however late within half an interval that one started.
        let mut due = start;
        for late in [0, 1, 4, 0, 3, 2] {
            due = next_due(due, due + ms(late), interval);
        }
        assert_eq!(due, start + 6 * interval);
        // A step that started further behind moves the rest of the schedule.
        assert_eq!(next_due(start, start + ms(6), interval), start + ms(16));
        assert_eq!(next_due(start, start + ms(45), interval), start + ms(55));
    }

    #[test]
    fn a_ramp_keeps_its_distance_from_the_one_before() {
        let interval = Duration::from_millis(10);
        let mut last_step = None;
        run(1.0, 0.9, interval, &mut last_step, |_, _| {});
        let previous = last_step.expect("the first ramp made a step");
        let mut first = None;
        run(0.9, 1.0, interval, &mut last_step, |_, _| {
            first.get_or_insert_with(Instant::now);
        });
        let first = first.expect("the second ramp made a step");
        assert!(first - previous >= interval, "{:?} apart", first - previous);
        assert!(last_step > Some(previous));
    }

    #[test]
    fn a_ramp_that_falls_behind_doesnt_catch_up() {
        let interval = Duration::from_millis(10);
        let mut starts = Vec::new();
        run(1.0, 0.5, interval, &mut None, |_, _| {
            starts.push(Instant::now());
            // The thread doesn't get to run for a while after the second step.
            if starts.len() == 2 {
                thread::sleep(interval * 4);
            }
        });
        assert!(starts.len() > 4);
        // Overdue steps aren't made back to back. The times here are taken a moment after each step starts, so they
        // are checked with room to spare.
        for pair in starts.windows(2) {
            assert!(pair[1] - pair[0] >= interval / 4, "steps {:?} apart", pair[1] - pair[0]);
        }
    }

    /// A session that remembers its volume and how often ducking read it.
    struct FakeSession {
        id: String,
        volume: Cell<f32>,
        reads: Cell<u32>,
    }

    impl FakeSession {
        fn new(id: &str, volume: f32) -> Self {
            Self {
                id: id.into(),
                volume: Cell::new(volume),
                reads: Cell::new(0),
            }
        }
    }

    impl Session for FakeSession {
        fn id(&self) -> &str {
            &self.id
        }

        fn identity(&self) -> Option<(String, f32)> {
            self.reads.set(self.reads.get() + 1);
            Some((format!("identifier of {}", self.id), self.volume.get()))
        }

        fn set_volume(&self, volume: f32) -> bool {
            self.volume.set(volume);
            true
        }
    }

    /// Ramps `sessions` like the Windows backend does, and returns the journal entries after each step.
    fn ramp(sessions: &[FakeSession], from: f32, to: f32, before: &mut BeforeDuck) -> Vec<Vec<JournalEntry>> {
        let mut journals = Vec::new();
        run(from, to, Duration::ZERO, &mut None, |level, duck_new| {
            apply(sessions, level, duck_new, before);
            journals.push(before.entries().cloned().collect());
        });
        journals
    }

    #[test]
    fn a_ramp_records_the_volume_from_before_ducking_once() {
        let sessions = [FakeSession::new("a", 0.8), FakeSession::new("b", 0.5)];
        let mut before = BeforeDuck::default();
        let journals = ramp(&sessions, 1.0, 0.25, &mut before);

        // Each session was read once, at the first step; every later step scales that volume.
        assert!(sessions.iter().all(|s| s.reads.get() == 1));
        for journal in &journals {
            let mut originals: Vec<f32> = journal.iter().map(|e| e.original).collect();
            originals.sort_by(f32::total_cmp);
            assert_eq!(originals, [0.5, 0.8]);
        }
        assert_eq!(sessions[0].volume.get(), 0.8 * 0.25);
        assert_eq!(sessions[1].volume.get(), 0.5 * 0.25);

        // Ducking deeper and restoring never read an intermediate volume either.
        ramp(&sessions, 0.25, 0.1, &mut before);
        ramp(&sessions, 0.1, 1.0, &mut before);
        assert!(sessions.iter().all(|s| s.reads.get() == 1));
        assert_eq!(sessions[0].volume.get(), 0.8);
        assert_eq!(sessions[1].volume.get(), 0.5);
        assert!(before.is_empty());
    }

    #[test]
    fn the_journal_follows_every_step() {
        let sessions = [FakeSession::new("a", 0.8)];
        let mut before = BeforeDuck::default();
        let journals = ramp(&sessions, 1.0, 0.25, &mut before);
        let ducked: Vec<f32> = journals.iter().map(|j| j[0].ducked).collect();
        assert_eq!(ducked.len(), 25);
        assert!(ducked.windows(2).all(|w| w[1] < w[0]), "{ducked:?}");
        assert_eq!(ducked.last(), Some(&(0.8 * 0.25)));
        // A restore ramp keeps the session in the journal until its last step.
        let journals = ramp(&sessions, 0.25, 1.0, &mut before);
        assert!(journals[..journals.len() - 1].iter().all(|j| j.len() == 1));
        assert_eq!(journals.last().map(Vec::len), Some(0));
    }

    #[test]
    fn a_ramp_up_doesnt_dip_sessions_that_arent_ducked() {
        let ducked = FakeSession::new("ducked", 1.0);
        let mut before = BeforeDuck::default();
        ramp(std::slice::from_ref(&ducked), 1.0, 0.25, &mut before);

        // The app started a session at full volume while ducked.
        let sessions = [ducked, FakeSession::new("new", 1.0)];
        let mut new_volumes = Vec::new();
        run(0.25, 0.5, Duration::ZERO, &mut None, |level, duck_new| {
            apply(&sessions, level, duck_new, &mut before);
            new_volumes.push(sessions[1].volume.get());
        });
        // It keeps its volume until the ramp's level is reached, then joins it.
        assert!(
            new_volumes[..new_volumes.len() - 1].iter().all(|&v| v == 1.0),
            "{new_volumes:?}"
        );
        assert_eq!(new_volumes.last(), Some(&0.5));
        assert_eq!(sessions[0].volume.get(), 0.5);

        // Restoring leaves sessions that were never ducked alone.
        let other = [FakeSession::new("other", 0.7)];
        ramp(&other, 0.5, 1.0, &mut before);
        assert_eq!(other[0].volume.get(), 0.7);
        assert_eq!(other[0].reads.get(), 0);
    }
}
