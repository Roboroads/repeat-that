//! The volume each ducked audio session had before ducking, so ducking never loses the user's own per-app volume.

// Only the Windows backend uses this so far; the tests run everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::HashMap;

/// Volumes from before ducking, by session instance id.
#[derive(Debug, Default)]
pub(crate) struct BeforeDuck(HashMap<String, f32>);

impl BeforeDuck {
    /// The volume to give session `id` for duck `level`, or `None` to leave it alone. The first duck remembers the
    /// session's `current` volume; later ducks scale that remembered volume, so they never compound. At `1.0` the
    /// remembered volume comes back, and sessions that were never ducked are left alone.
    pub fn target(&mut self, id: &str, level: f32, current: impl FnOnce() -> Option<f32>) -> Option<f32> {
        if level >= 1.0 {
            return self.0.get(id).copied();
        }
        let original = match self.0.get(id) {
            Some(&original) => original,
            None => {
                let current = current()?;
                self.0.insert(id.to_owned(), current);
                current
            }
        };
        Some(original * level)
    }

    /// Forgets session `id` once its volume from before ducking is back.
    pub fn restored(&mut self, id: &str) {
        self.0.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ducking_scales_the_volume_from_before_the_first_duck() {
        let mut before = BeforeDuck::default();
        assert_eq!(before.target("a", 0.5, || Some(0.8)), Some(0.4));
        // The session is at 0.4 now; ducking again must not compound.
        assert_eq!(before.target("a", 0.25, || Some(0.4)), Some(0.2));
        assert_eq!(before.target("a", 0.25, || Some(0.2)), Some(0.2));
    }

    #[test]
    fn unducking_restores_the_original_volume_until_it_is_confirmed() {
        let mut before = BeforeDuck::default();
        before.target("a", 0.5, || Some(0.8));
        assert_eq!(before.target("a", 1.0, || unreachable!()), Some(0.8));
        // Restoring failed: the next attempt still knows the original.
        assert_eq!(before.target("a", 1.0, || unreachable!()), Some(0.8));
        before.restored("a");
        assert_eq!(before.target("a", 1.0, || unreachable!()), None);
    }

    #[test]
    fn sessions_that_were_never_ducked_are_left_alone() {
        let mut before = BeforeDuck::default();
        assert_eq!(before.target("a", 1.0, || Some(0.8)), None);
    }

    #[test]
    fn sessions_whose_volume_cannot_be_read_are_skipped() {
        let mut before = BeforeDuck::default();
        assert_eq!(before.target("a", 0.5, || None), None);
        assert_eq!(before.target("a", 1.0, || unreachable!()), None);
    }
}
