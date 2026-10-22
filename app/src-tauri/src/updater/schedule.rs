//! When the app checks for updates by itself. Pure: times are [`Duration`]s since the app started, handed in by the
//! caller, so the tests run on a clock of their own.
//!
//! Automatic checks only happen once the user said yes (`checkForUpdates` is `true`) and only where there is something
//! to check for. Nothing about it is stored: each launch starts again with a first check.

use std::time::Duration;

/// How long to wait before each kind of check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timings {
    /// The first check after the app starts.
    pub first_check: Duration,
    /// Between two checks that worked.
    pub interval: Duration,
    /// After a check that failed, or that couldn't run because an install was under way.
    pub retry: Duration,
    /// After the user turns automatic checks on, if this run hasn't checked for a while.
    pub after_enabling: Duration,
}

impl Timings {
    pub const PRODUCTION: Timings = Timings {
        first_check: Duration::from_secs(60),
        interval: Duration::from_secs(24 * 60 * 60),
        retry: Duration::from_secs(60 * 60),
        after_enabling: Duration::from_secs(5),
    };
}

/// How a check that was started ended, as far as the schedule cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    /// A scheduled check, and whether it worked.
    Automatic { worked: bool },
    /// A check the user asked for, and whether it worked.
    Manual { worked: bool },
    /// The scheduled check had to be skipped: something else was running.
    Skipped,
}

pub struct Schedule {
    timings: Timings,
    /// Whether this install looks for updates at all (`install` and `notify` do).
    looks: bool,
    consent: Option<bool>,
    next: Option<Duration>,
    last_check: Option<Duration>,
}

impl Schedule {
    /// The schedule of an app that starts at `now` with the stored answer `consent`.
    pub fn new(timings: Timings, looks: bool, consent: Option<bool>, now: Duration) -> Self {
        let mut schedule = Schedule {
            timings,
            looks,
            consent,
            next: None,
            last_check: None,
        };
        if schedule.is_on() {
            schedule.next = Some(now + timings.first_check);
        }
        schedule
    }

    /// Whether automatic checks are on.
    pub fn is_on(&self) -> bool {
        self.looks && self.consent == Some(true)
    }

    /// When the next automatic check is due, `None` when there is none.
    pub fn next_due(&self) -> Option<Duration> {
        self.next
    }

    /// The user answered the question, or changed the answer.
    pub fn consent_changed(&mut self, consent: Option<bool>, now: Duration) {
        let was_on = self.is_on();
        self.consent = consent;
        match (was_on, self.is_on()) {
            (false, true) => {
                // A run that checked lately doesn't need to again at once.
                self.next = Some(match self.last_check {
                    Some(last) if now < last + self.timings.interval => last + self.timings.interval,
                    _ => now + self.timings.after_enabling,
                });
            }
            (true, false) => self.next = None,
            _ => {}
        }
    }

    /// A check ended at `now`.
    pub fn attempted(&mut self, attempt: Attempt, now: Duration) {
        let after = match attempt {
            Attempt::Automatic { worked: true } | Attempt::Manual { worked: true } => Some(self.timings.interval),
            Attempt::Automatic { worked: false } | Attempt::Skipped => Some(self.timings.retry),
            // The user's own failed check says nothing about when the next scheduled one should be.
            Attempt::Manual { worked: false } => None,
        };
        if attempt != Attempt::Skipped {
            self.last_check = Some(now);
        }
        if let (true, Some(after)) = (self.is_on(), after) {
            self.next = Some(now + after);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Timings = Timings::PRODUCTION;
    const MIN: Duration = Duration::from_secs(60);
    const HOUR: Duration = Duration::from_secs(3600);
    const DAY: Duration = Duration::from_secs(24 * 3600);

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    fn on() -> Schedule {
        Schedule::new(T, true, Some(true), Duration::ZERO)
    }

    #[test]
    fn the_production_timings_are_the_ones_decided() {
        assert_eq!(T.first_check, MIN);
        assert_eq!(T.interval, DAY);
        assert_eq!(T.retry, HOUR);
        assert_eq!(T.after_enabling, secs(5));
    }

    #[test]
    fn nothing_is_scheduled_until_the_user_says_yes() {
        for consent in [None, Some(false)] {
            let schedule = Schedule::new(T, true, consent, Duration::ZERO);
            assert!(!schedule.is_on(), "{consent:?}");
            assert_eq!(schedule.next_due(), None, "{consent:?}");
        }
    }

    #[test]
    fn nothing_is_scheduled_where_there_is_nothing_to_check() {
        let schedule = Schedule::new(T, false, Some(true), Duration::ZERO);
        assert!(!schedule.is_on());
        assert_eq!(schedule.next_due(), None);
    }

    #[test]
    fn the_first_check_is_a_minute_after_the_app_starts() {
        assert_eq!(on().next_due(), Some(MIN));
        let late = Schedule::new(T, true, Some(true), secs(10));
        assert_eq!(late.next_due(), Some(secs(70)));
    }

    #[test]
    fn a_check_that_worked_is_followed_by_one_a_day_later() {
        let mut schedule = on();
        schedule.attempted(Attempt::Automatic { worked: true }, MIN);
        assert_eq!(schedule.next_due(), Some(MIN + DAY));
        schedule.attempted(Attempt::Automatic { worked: true }, MIN + DAY + secs(3));
        assert_eq!(schedule.next_due(), Some(MIN + DAY + secs(3) + DAY));
    }

    #[test]
    fn a_check_that_failed_is_tried_again_in_an_hour() {
        let mut schedule = on();
        schedule.attempted(Attempt::Automatic { worked: false }, MIN);
        assert_eq!(schedule.next_due(), Some(MIN + HOUR));
        schedule.attempted(Attempt::Automatic { worked: false }, MIN + HOUR);
        assert_eq!(schedule.next_due(), Some(MIN + HOUR + HOUR), "and again");
        schedule.attempted(Attempt::Automatic { worked: true }, MIN + 2 * HOUR);
        assert_eq!(schedule.next_due(), Some(MIN + 2 * HOUR + DAY), "until one works");
    }

    #[test]
    fn a_skipped_check_is_tried_again_in_an_hour() {
        let mut schedule = on();
        schedule.attempted(Attempt::Skipped, MIN);
        assert_eq!(schedule.next_due(), Some(MIN + HOUR));
    }

    #[test]
    fn a_manual_check_that_worked_moves_the_next_one_a_day_out() {
        let mut schedule = on();
        schedule.attempted(Attempt::Manual { worked: true }, secs(30));
        assert_eq!(schedule.next_due(), Some(secs(30) + DAY));
    }

    #[test]
    fn a_manual_check_that_failed_leaves_the_schedule_alone() {
        let mut schedule = on();
        schedule.attempted(Attempt::Manual { worked: false }, secs(30));
        assert_eq!(schedule.next_due(), Some(MIN));
    }

    #[test]
    fn a_manual_check_schedules_nothing_while_automatic_checks_are_off() {
        for consent in [None, Some(false)] {
            let mut schedule = Schedule::new(T, true, consent, Duration::ZERO);
            schedule.attempted(Attempt::Manual { worked: true }, secs(30));
            assert_eq!(schedule.next_due(), None, "{consent:?}");
        }
    }

    #[test]
    fn saying_yes_checks_after_five_seconds_when_this_run_has_not_checked() {
        let mut schedule = Schedule::new(T, true, None, Duration::ZERO);
        schedule.consent_changed(Some(true), secs(100));
        assert!(schedule.is_on());
        assert_eq!(schedule.next_due(), Some(secs(105)));
    }

    #[test]
    fn saying_yes_a_day_after_the_last_check_checks_after_five_seconds() {
        let mut schedule = Schedule::new(T, true, Some(false), Duration::ZERO);
        schedule.attempted(Attempt::Manual { worked: true }, secs(30));
        schedule.consent_changed(Some(true), secs(30) + DAY);
        assert_eq!(schedule.next_due(), Some(secs(30) + DAY + secs(5)));
    }

    #[test]
    fn saying_yes_soon_after_a_check_waits_for_the_day_to_pass() {
        let mut schedule = Schedule::new(T, true, Some(false), Duration::ZERO);
        schedule.attempted(Attempt::Manual { worked: true }, secs(30));
        schedule.consent_changed(Some(true), secs(600));
        assert_eq!(schedule.next_due(), Some(secs(30) + DAY));
    }

    #[test]
    fn saying_no_stops_the_schedule() {
        let mut schedule = on();
        schedule.consent_changed(Some(false), secs(10));
        assert!(!schedule.is_on());
        assert_eq!(schedule.next_due(), None);
        schedule.attempted(Attempt::Automatic { worked: true }, secs(20));
        assert_eq!(
            schedule.next_due(),
            None,
            "a check that was already running doesn't bring it back"
        );
    }

    #[test]
    fn saying_the_same_thing_again_changes_nothing() {
        let mut schedule = on();
        schedule.consent_changed(Some(true), secs(40));
        assert_eq!(schedule.next_due(), Some(MIN), "the first check stays where it was");
        let mut off = Schedule::new(T, true, Some(false), Duration::ZERO);
        off.consent_changed(Some(false), secs(40));
        assert_eq!(off.next_due(), None);
    }

    #[test]
    fn going_back_to_not_asked_counts_as_off() {
        let mut schedule = on();
        schedule.consent_changed(None, secs(10));
        assert!(!schedule.is_on());
        assert_eq!(schedule.next_due(), None);
    }

    #[test]
    fn where_there_is_nothing_to_check_saying_yes_schedules_nothing() {
        let mut schedule = Schedule::new(T, false, None, Duration::ZERO);
        schedule.consent_changed(Some(true), secs(10));
        assert_eq!(schedule.next_due(), None);
    }
}
