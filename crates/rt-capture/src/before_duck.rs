//! The volume each ducked audio session had before ducking, so ducking never loses the user's own per-app volume.

// Only the Windows backend uses this so far; the tests run everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::HashMap;

use crate::duck_journal::JournalEntry;

/// Ducked sessions with their volume from before ducking, by session instance id.
#[derive(Debug, Default)]
pub(crate) struct BeforeDuck(HashMap<String, JournalEntry>);

impl BeforeDuck {
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

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Forgets session `id` once its volume from before ducking is back.
    pub fn restored(&mut self, id: &str) {
        self.0.remove(id);
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
        before.target("a", 0.5, session(0.8));
        assert!(before.contains("a"));
        before.restored("a");
        assert!(!before.contains("a"));
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
