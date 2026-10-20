//! A journal of the sessions that are ducked right now, so their volume can be restored after a crash. Windows remembers
//! per-app volumes, so an app that was ducked when we died would otherwise stay quiet for good.
//!
//! The journal only holds volume settings and session identifiers, never audio.
//!
//! One line per session: `<original volume>\t<ducked volume>\t<session identifier>`.

// Only the Windows backend uses this so far; the tests run everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::fs;
use std::io;
use std::path::Path;

/// How far a session's volume may be from the ducked volume we set and still count as untouched.
const SAME_VOLUME: f32 = 1e-3;

/// A ducked session.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JournalEntry {
    /// The session identifier, which stays the same when the app restarts (unlike the session instance id).
    pub identifier: String,
    /// The volume from before ducking.
    pub original: f32,
    /// The volume we set.
    pub ducked: f32,
}

/// The journal text for `entries`. Entries whose identifier would break the format are left out.
pub(crate) fn serialize<'a>(entries: impl IntoIterator<Item = &'a JournalEntry>) -> String {
    entries
        .into_iter()
        .filter(|e| !e.identifier.is_empty() && !e.identifier.contains(['\t', '\n', '\r']))
        .map(|e| format!("{}\t{}\t{}\n", e.original, e.ducked, e.identifier))
        .collect()
}

/// The entries in a journal, or `None` if it is corrupt.
pub(crate) fn parse(text: &str) -> Option<Vec<JournalEntry>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.splitn(3, '\t');
            let original = volume(fields.next()?)?;
            let ducked = volume(fields.next()?)?;
            let identifier = fields.next().filter(|id| !id.is_empty())?.to_owned();
            Some(JournalEntry {
                identifier,
                original,
                ducked,
            })
        })
        .collect()
}

fn volume(field: &str) -> Option<f32> {
    field.parse().ok().filter(|v| (0.0..=1.0).contains(v))
}

/// Whether a session from the journal, now at `current`, should get its original volume back. If its volume is no
/// longer the one we set, the user changed it by hand, and it is theirs to keep.
pub(crate) fn should_restore(entry: &JournalEntry, current: f32) -> bool {
    (current - entry.ducked).abs() <= SAME_VOLUME
}

/// Reads the journal at `path`. A missing journal is empty; a corrupt or unreadable one is removed and treated as empty.
pub(crate) fn read(path: &Path) -> Vec<JournalEntry> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).unwrap_or_else(|| {
            let _ = fs::remove_file(path);
            Vec::new()
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(_) => {
            let _ = fs::remove_file(path);
            Vec::new()
        }
    }
}

/// Replaces the journal at `path` with `text` in one step, so a crash never leaves half a journal. Empty text removes
/// it.
pub(crate) fn write(path: &Path, text: &str) -> io::Result<()> {
    if text.is_empty() {
        return match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    fs::write(&temp, text)?;
    fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(identifier: &str, original: f32, ducked: f32) -> JournalEntry {
        JournalEntry {
            identifier: identifier.into(),
            original,
            ducked,
        }
    }

    #[test]
    fn entries_survive_a_round_trip() {
        let entries = vec![
            entry(
                r"{0.0.0.00000000}.{guid}|\Device\HarddiskVolume3\Discord.exe%b{guid}",
                0.8,
                0.04,
            ),
            entry("other", 1.0, 0.123_456_79),
        ];
        assert_eq!(parse(&serialize(&entries)), Some(entries));
    }

    #[test]
    fn identifiers_that_would_break_the_format_are_left_out() {
        let entries = [entry("a\tb", 1.0, 0.5), entry("a\nb", 1.0, 0.5), entry("", 1.0, 0.5)];
        assert_eq!(serialize(&entries), "");
    }

    #[test]
    fn corrupt_journals_are_rejected() {
        assert_eq!(parse(""), Some(vec![]));
        assert_eq!(parse("0.5\t0.1"), None);
        assert_eq!(parse("0.5\t0.1\t"), None);
        assert_eq!(parse("loud\t0.1\tid"), None);
        assert_eq!(parse("2\t0.1\tid"), None);
        assert_eq!(parse("NaN\t0.1\tid"), None);
        assert_eq!(parse("0.5\t0.1\tid\n\u{0}garbage"), None);
    }

    #[test]
    fn restores_only_sessions_still_at_the_ducked_volume() {
        let ducked = entry("id", 0.8, 0.04);
        assert!(should_restore(&ducked, 0.04));
        assert!(should_restore(&ducked, 0.0405));
        // The user turned it up (or down) by hand meanwhile.
        assert!(!should_restore(&ducked, 0.5));
        assert!(!should_restore(&ducked, 0.02));
    }

    #[test]
    fn writes_reads_and_removes_the_journal() {
        let dir = std::env::temp_dir().join(format!("rt-duck-journal-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("duck.journal");
        let entries = vec![entry("id", 0.8, 0.04)];

        write(&path, &serialize(&entries)).unwrap();
        assert_eq!(read(&path), entries);
        write(&path, "").unwrap();
        assert!(!path.exists());
        assert_eq!(read(&path), vec![]);
        write(&path, "").unwrap();

        fs::write(&path, "not a journal").unwrap();
        assert_eq!(read(&path), vec![]);
        assert!(!path.exists(), "corrupt journals are removed");
        fs::remove_dir_all(&dir).unwrap();
    }
}
