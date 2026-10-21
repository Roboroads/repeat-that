//! macOS backend: Core Audio process taps (macOS 14.2+) for capture and for ducking.
//!
//! An app is the group of processes macOS holds responsible to it (see `processes`), named after its outermost `.app`
//! bundle. A capture taps the group's audio through a private process tap in an aggregate device on the app's own output
//! device (see `tap`). macOS has no per-app volume, so ducking mutes the app through a second, muting tap and plays
//! what the first one records on that device ourselves, at the duck level (see `passthrough`). Taps record before the
//! mute, so the capture keeps its full level, and private taps go away with this process: no duck journal or guard is
//! needed.
//!
//! Capturing other apps needs the user's permission (see `permission`). The parts that are pure code, like grouping
//! processes and the ducking state machine, also build in tests on other OSes, so their tests run everywhere.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

mod icon;
mod passthrough;
pub(crate) mod permission;
mod processes;
#[cfg(target_os = "macos")]
mod sys;
#[cfg(target_os = "macos")]
mod tap;

use crate::{CaptureError, Result};

#[cfg(target_os = "macos")]
pub use self::os::*;

/// The first macOS with process taps.
const FIRST_VERSION: (i64, i64) = (14, 2);

/// Whether macOS `major`.`minor` can capture apps.
fn check_version(major: i64, minor: i64) -> Result<()> {
    if (major, minor) >= FIRST_VERSION {
        Ok(())
    } else {
        Err(CaptureError::Unsupported(format!(
            "Capturing apps needs macOS {}.{} or later",
            FIRST_VERSION.0, FIRST_VERSION.1
        )))
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::{Arc, Mutex, MutexGuard, Weak};

    use super::{check_version, processes, sys, tap};
    use crate::{AppAudioSource, AudioApp, CaptureHandle, FrameSink, IconImage, PlatformOptions, Result};

    pub(crate) const HAS_BACKEND: bool = true;
    /// Taps are private to this process, so macOS removes them, and their mute, when it dies.
    pub(crate) const HAS_DUCK_GUARD: bool = false;
    /// Paths come from the OS as they are on disk, so they compare as they are, even on a disk that ignores case.
    pub(crate) const EXE_PATHS_IGNORE_CASE: bool = false;

    /// The Core Audio source. macOS removes our taps when we die, so there is nothing for a duck journal to do.
    pub(crate) fn source(_options: PlatformOptions) -> Result<Box<dyn AppAudioSource>> {
        let (major, minor) = sys::os_version();
        check_version(major, minor)?;
        Ok(Box::new(CoreAudioSource::new()))
    }

    pub(crate) fn icon(exe: &Path, size_px: u32) -> Option<IconImage> {
        super::icon::bundle_icon(exe, size_px)
    }

    /// There is no guard here (`HAS_DUCK_GUARD` is false), so nothing ever starts one: `false` leaves a request for one
    /// to the app.
    pub(crate) fn run_duck_guard(_journal: &Path, _parent: u32) -> bool {
        false
    }

    /// The process macOS holds responsible for `pid`, e.g. Safari for its WebKit GPU process, or the terminal a tool
    /// runs in. `None` if macOS doesn't say.
    pub fn responsible_pid(pid: u32) -> Option<u32> {
        sys::responsible_pid(pid)
    }

    /// Captures and ducks apps through Core Audio process taps (macOS 14.2+).
    ///
    /// Each app has one tap at a time, shared by its capture and its duck. Ducking mutes the app through a second tap
    /// and plays the tapped audio on the app's own device at the duck level, so captures keep the full level. Taps are
    /// private: macOS removes them, and their mute, when this process ends, also when it crashes.
    #[derive(Default)]
    pub struct CoreAudioSource {
        /// The tap of each app, by PID, while a capture or a duck holds it.
        taps: Mutex<HashMap<u32, Weak<tap::Tap>>>,
        /// Taps of apps that are ducked, held so the duck lasts without a capture.
        ducked: Mutex<HashMap<u32, Arc<tap::Tap>>>,
    }

    impl CoreAudioSource {
        pub fn new() -> Self {
            Self::default()
        }

        /// The running tap of `app`, or a new one. A new tap waits for its device to run, which mustn't hold up other
        /// apps' captures and ducks, so the list isn't locked meanwhile.
        fn tap(&self, app: &AudioApp) -> Result<Arc<tap::Tap>> {
            let running = |taps: &mut HashMap<u32, Weak<tap::Tap>>| {
                taps.retain(|_, tap| tap.strong_count() > 0);
                taps.get(&app.pid).and_then(Weak::upgrade).filter(|tap| !tap.ended())
            };
            let found = running(&mut lock(&self.taps));
            if let Some(tap) = found {
                return Ok(tap);
            }
            let tap = Arc::new(tap::Tap::start(app.pid)?);
            let mut taps = lock(&self.taps);
            if let Some(started) = running(&mut taps) {
                // Another capture or duck of the app started one meanwhile: that one it is, and ours goes.
                drop(taps);
                return Ok(started);
            }
            taps.insert(app.pid, Arc::downgrade(&tap));
            Ok(tap)
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl AppAudioSource for CoreAudioSource {
        fn list_apps(&self) -> Result<Vec<AudioApp>> {
            let processes = sys::audio_processes()?;
            let groups = processes::group(&processes, std::process::id(), &sys::process_path, &sys::display_name);
            Ok(groups.into_iter().filter(|g| g.listed()).map(|g| g.app).collect())
        }

        fn start(&self, app: &AudioApp, sink: FrameSink) -> Result<Box<dyn CaptureHandle>> {
            super::permission::check()?;
            let tap = self.tap(app)?;
            Ok(Box::new(tap::Capture::new(tap, sink)))
        }

        fn set_app_volume(&self, app: &AudioApp, level: f32) -> Result<()> {
            let level = if level.is_nan() { 1.0 } else { level.clamp(0.0, 1.0) };
            // The list is only locked to look and to update it: a duck takes a few hundred milliseconds, seconds when
            // its device stalls, which mustn't hold up other apps' ducks and restores. Ducks of one app take turns in
            // its tap. Taps that go are dropped after unlocking, since dropping one waits for its watcher.
            let (held, ended) = {
                let mut ducked = lock(&self.ducked);
                // Taps of other apps that ended by themselves were unmuted then; this app's is restored below.
                let ended: Vec<_> = ducked.extract_if(|pid, tap| *pid != app.pid && tap.ended()).collect();
                (ducked.get(&app.pid).cloned(), ended)
            };
            drop(ended);
            let tap = match held {
                // Restoring always goes to the tap that ducked the app, also one that ended by itself while a capture
                // still holds it: that unmutes the app and stops the replay, if its end didn't already.
                Some(tap) if level >= 1.0 || !tap.ended() => tap,
                // Nothing to restore on an app that isn't ducked.
                None if level >= 1.0 => return Ok(()),
                _ => {
                    super::permission::check()?;
                    self.tap(app)?
                }
            };
            let result = tap.duck(level);
            // Held while the tap keeps the app ducked. A duck that failed left the app as it was before ducking (see
            // `Tap::duck`). Of two ducks of the app at once, the one that updates last sees how the other left it.
            let gone = {
                let mut ducked = lock(&self.ducked);
                if tap.ducked() {
                    ducked.insert(app.pid, tap)
                } else if ducked.get(&app.pid).is_some_and(|held| Arc::ptr_eq(held, &tap)) {
                    ducked.remove(&app.pid)
                } else {
                    None
                }
            };
            drop(gone);
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_need_macos_14_2() {
        for (major, minor) in [(13, 6), (14, 0), (14, 1)] {
            match check_version(major, minor) {
                Err(CaptureError::Unsupported(why)) => {
                    assert_eq!(why, "Capturing apps needs macOS 14.2 or later");
                }
                other => panic!("{major}.{minor}: {other:?}"),
            }
        }
        for (major, minor) in [(14, 2), (14, 6), (15, 0), (26, 0)] {
            assert!(check_version(major, minor).is_ok(), "{major}.{minor}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn it_has_a_backend_but_no_guard() {
        const { assert!(HAS_BACKEND) };
        const { assert!(!HAS_DUCK_GUARD) };
        const { assert!(crate::HAS_PLATFORM_BACKEND) };
        assert!(!run_duck_guard(std::path::Path::new("duck.journal"), 1234));
    }
}
