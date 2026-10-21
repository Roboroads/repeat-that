//! The duck guard: a silent second process that restores ducked apps when the app dies without doing so itself, like a
//! crash handler. It waits for the app to exit, then restores the sessions the duck journal still lists. After a normal
//! exit the journal is empty and the guard just ends. It only touches volumes, never audio.
//!
//! The guard is the app's own executable, started with hidden arguments, so the app must call [`run_if_requested`]
//! first thing in `main`. Only the Windows backend starts it (see `WasapiSource::with_duck_guard`) for now.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The hidden argument that makes the app run as a duck guard: `--rt-duck-guard <journal path> <parent pid>`.
const GUARD_ARG: &str = "--rt-duck-guard";

/// Runs the duck guard if this process was started as one, and returns `true` once it is done: the caller should then
/// exit. Returns `false` right away otherwise, and always on OSes without a guard.
pub fn run_if_requested() -> bool {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    run(&args)
}

/// [`run_if_requested`] for the arguments `args` (without the program name). The OS decides what a guard is and
/// whether it has one, see `run_duck_guard` in its module.
fn run(args: &[OsString]) -> bool {
    match requested(args) {
        Some((journal, parent)) => crate::os::run_duck_guard(&journal, parent),
        None => false,
    }
}

/// The arguments that start a guard for `journal` that waits for process `parent`.
#[cfg_attr(not(windows), allow(dead_code))]
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

    fn strings(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn ordinary_arguments_are_not_a_guard() {
        let args = strings;
        assert_eq!(requested(&args(&[])), None);
        assert_eq!(requested(&args(&["list"])), None);
        assert_eq!(requested(&args(&["--rt-duck-guard", "journal"])), None);
        assert_eq!(requested(&args(&["--rt-duck-guard", "journal", "not a pid"])), None);
        assert_eq!(requested(&args(&["--other", "journal", "1234"])), None);
        assert_eq!(requested(&args(&["--rt-duck-guard", "journal", "1234", "extra"])), None);
    }

    #[test]
    fn run_if_requested_is_false_without_guard_arguments_on_every_os() {
        assert!(!run(&[]));
        assert!(!run(&strings(&["list"])));
        assert!(!run(&strings(&["--rt-duck-guard", "journal"])));
        // The arguments of this test run are no guard request either.
        assert!(!run_if_requested());
    }

    #[test]
    fn an_os_without_a_guard_never_runs_one() {
        if crate::HAS_PLATFORM_BACKEND {
            return;
        }
        // Nothing starts a guard there, so a request for one is left to the app instead of ending it.
        assert!(!run(&strings(&["--rt-duck-guard", "journal", "1234"])));
    }
}
