//! The volume each ducked audio session had before ducking, so ducking never loses the user's own per-app volume.

// Only the Windows and Linux backends use this; the tests run everywhere.
#![cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]

use std::collections::HashMap;

use crate::duck_journal::JournalEntry;

/// Ducked sessions with their volume from before ducking, by session instance id.
#[derive(Debug, Default)]
pub(crate) struct BeforeDuck(HashMap<String, JournalEntry>);

/// An audio session whose volume ducking changes.
pub(crate) trait Session {
    /// Tells sessions apart while they live: the session instance id on Windows.
    fn id(&self) -> &str;

    /// The session identifier, which stays the same when the app restarts, and the current volume. `None` if they
    /// can't be read.
    fn identity(&self) -> Option<(String, f32)>;

    /// Sets the session's volume; `false` if that failed.
    fn set_volume(&self, volume: f32) -> bool;
}

impl BeforeDuck {
    /// Gives `session` duck `level`, see [`target`](Self::target), and forgets it once it has its volume from before
    /// ducking back.
    pub fn apply(&mut self, session: &impl Session, level: f32) {
        let Some(target) = self.target(session.id(), level, || session.identity()) else {
            return;
        };
        if session.set_volume(target) && level >= 1.0 {
            self.restored(session.id());
        }
    }

    /// The volume to give session `id` for duck `level`, or `None` to leave it alone. The first duck remembers the
    /// session's identifier and `current` volume from `session`; later ducks scale that remembered volume, so they
    /// never compound. At `1.0` the remembered volume comes back, and sessions that were never ducked are left alone.
    pub fn target(&mut self, id: &str, level: f32, session: impl FnOnce() -> Option<(String, f32)>) -> Option<f32> {
        if level >= 1.0 {
            return self.0.get(id).map(|e| e.original);
        }
        let entry = match self.0.get_mut(id) {
            Some(entry) => entry,
            None => {
                let (identifier, current) = session()?;
                let entry = JournalEntry {
                    identifier,
                    original: current,
                    ducked: current,
                };
                self.0.entry(id.to_owned()).or_insert(entry)
            }
        };
        entry.ducked = entry.original * level;
        Some(entry.ducked)
    }

    /// Whether session `id` is ducked.
    pub fn contains(&self, id: &str) -> bool {
        self.0.contains_key(id)
    }

    /// The volumes of session `id` from before ducking and from the latest duck, if it is ducked.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn get(&self, id: &str) -> Option<&JournalEntry> {
        self.0.get(id)
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Takes session `id` as ducked, as `entry` says, without changing its volume: a session that got a ducked volume
    /// from elsewhere, like a new stream that Linux's session manager gives the volume it remembers from a duck of
    /// another one. A session that is ducked already keeps what it has.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn adopt(&mut self, id: &str, entry: JournalEntry) {
        self.0.entry(id.to_owned()).or_insert(entry);
    }

    /// Forgets session `id` once its volume from before ducking is back.
    pub fn restored(&mut self, id: &str) {
        self.0.remove(id);
    }

    /// Forgets session `id`, which is gone, and gives what it was ducked from, if it was ducked.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn take(&mut self, id: &str) -> Option<JournalEntry> {
        self.0.remove(id)
    }

    /// The ducked sessions, for the duck journal.
    pub fn entries(&self) -> impl Iterator<Item = &JournalEntry> {
        self.0.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(current: f32) -> impl FnOnce() -> Option<(String, f32)> {
        move || Some(("app".into(), current))
    }

    #[test]
    fn ducking_scales_the_volume_from_before_the_first_duck() {
        let mut before = BeforeDuck::default();
        assert_eq!(before.target("a", 0.5, session(0.8)), Some(0.4));
        // The session is at 0.4 now; ducking again must not compound.
        assert_eq!(before.target("a", 0.25, session(0.4)), Some(0.2));
        assert_eq!(before.target("a", 0.25, session(0.2)), Some(0.2));
    }

    #[test]
    fn unducking_restores_the_original_volume_until_it_is_confirmed() {
        let mut before = BeforeDuck::default();
        before.target("a", 0.5, session(0.8));
        assert_eq!(before.target("a", 1.0, || unreachable!()), Some(0.8));
        // Restoring failed: the next attempt still knows the original.
        assert_eq!(before.target("a", 1.0, || unreachable!()), Some(0.8));
        before.restored("a");
        assert_eq!(before.target("a", 1.0, || unreachable!()), None);
    }

    #[test]
    fn sessions_that_were_never_ducked_are_left_alone() {
        let mut before = BeforeDuck::default();
        assert_eq!(before.target("a", 1.0, session(0.8)), None);
    }

    #[test]
    fn ducked_sessions_are_known_until_restored() {
        let mut before = BeforeDuck::default();
        assert!(!before.contains("a"));
        assert_eq!(before.get("a"), None);
        before.target("a", 0.5, session(0.8));
        assert!(before.contains("a"));
        assert_eq!(before.get("a").map(|e| (e.original, e.ducked)), Some((0.8, 0.4)));
        before.restored("a");
        assert!(!before.contains("a"));
    }

    #[test]
    fn an_adopted_session_is_restored_to_the_original_it_was_given() {
        let mut before = BeforeDuck::default();
        before.adopt(
            "b",
            JournalEntry {
                identifier: "app".into(),
                original: 0.8,
                ducked: 0.2,
            },
        );
        assert!(before.contains("b"));
        // Its volume isn't read again, and later ducks scale the original it was given.
        assert_eq!(before.target("b", 0.5, || unreachable!()), Some(0.4));
        assert_eq!(before.target("b", 1.0, || unreachable!()), Some(0.8));
    }

    #[test]
    fn a_session_that_is_gone_gives_its_entry() {
        let mut before = BeforeDuck::default();
        before.target("a", 0.5, session(0.8));
        let entry = before.take("a").unwrap();
        assert_eq!((entry.original, entry.ducked), (0.8, 0.4));
        assert!(!before.contains("a"));
        assert_eq!(before.take("a"), None);
    }

    #[test]
    fn sessions_whose_volume_cannot_be_read_are_skipped() {
        let mut before = BeforeDuck::default();
        assert_eq!(before.target("a", 0.5, || None), None);
        assert_eq!(before.target("a", 1.0, || unreachable!()), None);
    }

    #[test]
    fn journal_entries_hold_the_original_and_the_ducked_volume() {
        let mut before = BeforeDuck::default();
        before.target("a", 0.5, session(0.8));
        before.target("a", 0.25, session(0.4));
        let entries: Vec<_> = before.entries().cloned().collect();
        assert_eq!(
            entries,
            [JournalEntry {
                identifier: "app".into(),
                original: 0.8,
                ducked: 0.2
            }]
        );
    }
}
