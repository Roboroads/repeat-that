//! The duck guard: a silent second process that restores ducked apps when the app dies without doing so itself, like a
//! crash handler. It waits for the app to exit, then restores the sessions the duck journal still lists. After a normal
//! exit the journal is empty and the guard just ends. It only touches volumes, never audio.
//!
//! The guard is the app's own executable, started with hidden arguments, so the app must call [`run_if_requested`]
//! first thing in `main`. Only the Windows backend starts it (see `WasapiSource::with_duck_guard`).

// Only the Windows backend uses some of this; the tests run everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The hidden argument that makes the app run as a duck guard: `--rt-duck-guard <journal path> <parent pid>`.
const GUARD_ARG: &str = "--rt-duck-guard";

/// Runs the duck guard if this process was started as one, and returns `true` once it is done: the caller should then
/// exit. Returns `false` right away otherwise, and always on OSes without a guard.
pub fn run_if_requested() -> bool {
    #[cfg(windows)]
    {
        let args: Vec<OsString> = std::env::args_os().skip(1).collect();
        match requested(&args) {
            Some((journal, parent)) => {
                crate::windows::run_duck_guard(&journal, parent);
                true
            }
            None => false,
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// The arguments that start a guard for `journal` that waits for process `parent`.
pub(crate) fn args(journal: &Path, parent: u32) -> [OsString; 3] {
    [GUARD_ARG.into(), journal.into(), parent.to_string().into()]
}

/// The journal and parent PID if `args` (without the program name) start a guard.
fn requested(args: &[OsString]) -> Option<(PathBuf, u32)> {
    match args {
        [flag, journal, parent] if flag == OsStr::new(GUARD_ARG) => {
            Some((journal.into(), parent.to_str()?.parse().ok()?))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_arguments_round_trip() {
        let journal = Path::new("C:\\Users\\me\\AppData\\Roaming\\repeat-that\\duck.journal");
        assert_eq!(requested(&args(journal, 1234)), Some((journal.to_owned(), 1234)));
    }

    #[test]
    fn ordinary_arguments_are_not_a_guard() {
        let args = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(requested(&args(&[])), None);
        assert_eq!(requested(&args(&["list"])), None);
        assert_eq!(requested(&args(&["--rt-duck-guard", "journal"])), None);
        assert_eq!(requested(&args(&["--rt-duck-guard", "journal", "not a pid"])), None);
        assert_eq!(requested(&args(&["--other", "journal", "1234"])), None);
        assert_eq!(requested(&args(&["--rt-duck-guard", "journal", "1234", "extra"])), None);
    }
}
