//! The permission to capture other apps' audio (TCC service `kTCCServiceAudioCapture`), which the user gives in System
//! Settings > Privacy & Security > Screen & System Audio Recording (macOS 15; 14.x only shows the prompt).
//!
//! macOS has no public function that tells whether the permission was given. `TCCAccessPreflight` and
//! `TCCAccessRequest` from the private TCC framework do, and are loaded at run time. If they are missing, the app
//! assumes it may capture and goes on: detection is simply off (a written assumption, see the plan).

use crate::permission::{PermissionHelp, PermissionStatus};
use crate::{CaptureError, Result};

/// What `TCCAccessPreflight` returned, or `None` if it isn't there.
pub(crate) fn from_preflight(value: Option<i32>) -> PermissionStatus {
    match value {
        None | Some(0) => PermissionStatus::Allowed,
        Some(1) => PermissionStatus::Denied,
        Some(_) => PermissionStatus::NotAsked,
    }
}

/// What the user can do on macOS `major`: System Settings has the switch from macOS 15 on; 14.x only asks.
pub(crate) fn help_for(major: i64) -> PermissionHelp {
    PermissionHelp {
        can_open_settings: major >= 15,
        can_ask_again: true,
    }
}

/// How to allow capture on macOS `major`, as a sentence for the user.
pub(crate) fn how_to_allow(major: i64) -> String {
    if major >= 15 {
        "Turn on Repeat That in System Settings > Privacy & Security > Screen & System Audio Recording.".into()
    } else {
        "Click Ask again and choose Allow.".into()
    }
}

/// The capture error, if any, for `status`, after asking the user if they weren't yet.
pub(crate) fn start_error(status: PermissionStatus, major: i64, ask: impl FnOnce()) -> Result<()> {
    match status {
        PermissionStatus::Allowed => Ok(()),
        PermissionStatus::NotAsked => {
            // macOS shows its question and doesn't wait for the answer. The engine tries again every 2 s, so capture
            // starts once the user allows it.
            ask();
            Err(CaptureError::Backend(WAITING.into()))
        }
        PermissionStatus::Denied => Err(CaptureError::PermissionDenied(how_to_allow(major))),
    }
}

/// The error while macOS asks the user.
pub(crate) const WAITING: &str = "Waiting for you to allow audio capture in the macOS dialog";

#[cfg(target_os = "macos")]
pub(crate) use self::sys::{ask_again, check, help, open_settings, status};

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{CStr, c_int, c_void};
    use std::process::Command;
    use std::sync::OnceLock;

    use block2::{Block, RcBlock};
    use objc2_core_foundation::{CFDictionary, CFString};
    use objc2_foundation::NSBundle;

    use super::{from_preflight, help_for, start_error};
    use crate::macos::{check_version, sys::os_version};
    use crate::permission::{PermissionHelp, PermissionStatus};
    use crate::{CaptureError, Result};

    const TCC: &CStr = c"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC";
    const SERVICE: &str = "kTCCServiceAudioCapture";
    /// Privacy & Security. Deeper links into it aren't documented, and nobody could try them.
    const SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.security";

    type Preflight = unsafe extern "C" fn(*const CFString, *const CFDictionary) -> c_int;
    type Request = unsafe extern "C" fn(*const CFString, *const CFDictionary, *const Block<dyn Fn(u8)>);

    /// The two functions of the private TCC framework, if it has them.
    struct Tcc {
        preflight: Option<Preflight>,
        request: Option<Request>,
    }

    fn tcc() -> &'static Tcc {
        static TCC_FUNCTIONS: OnceLock<Tcc> = OnceLock::new();
        TCC_FUNCTIONS.get_or_init(|| {
            // SAFETY: a system framework, loaded once and never closed; the symbols have the types above (as used by
            // other apps; they are private and undocumented).
            unsafe {
                let handle = libc::dlopen(TCC.as_ptr(), libc::RTLD_NOW);
                if handle.is_null() {
                    return Tcc {
                        preflight: None,
                        request: None,
                    };
                }
                let symbol = |name: &CStr| {
                    let symbol = libc::dlsym(handle, name.as_ptr());
                    (!symbol.is_null()).then_some(symbol)
                };
                Tcc {
                    preflight: symbol(c"TCCAccessPreflight").map(|f| std::mem::transmute::<*mut c_void, Preflight>(f)),
                    request: symbol(c"TCCAccessRequest").map(|f| std::mem::transmute::<*mut c_void, Request>(f)),
                }
            }
        })
    }

    fn major() -> Result<i64> {
        let (major, minor) = os_version();
        check_version(major, minor)?;
        Ok(major)
    }

    pub(crate) fn status() -> Result<PermissionStatus> {
        major()?;
        let service = CFString::from_static_str(SERVICE);
        let value = tcc()
            .preflight
            // SAFETY: a valid string and no options, as the function takes them.
            .map(|preflight| unsafe { preflight(&*service, std::ptr::null()) });
        Ok(from_preflight(value))
    }

    /// Makes macOS show its question, without waiting for the answer.
    fn request() {
        let Some(request) = tcc().request else {
            return;
        };
        let service = CFString::from_static_str(SERVICE);
        let answered = RcBlock::new(|_granted: u8| {});
        // SAFETY: a valid string, no options, and a block that lives as long as TCC keeps it (it copies it).
        unsafe { request(&*service, std::ptr::null(), &*answered) };
    }

    /// Whether a capture may start: allowed, or the error to start with (see `start_error`).
    pub(crate) fn check() -> Result<()> {
        let major = major()?;
        start_error(status()?, major, request)
    }

    pub(crate) fn help() -> Result<PermissionHelp> {
        Ok(help_for(major()?))
    }

    pub(crate) fn ask_again() -> Result<()> {
        major()?;
        let bundle_id = NSBundle::mainBundle().bundleIdentifier().map(|id| id.to_string());
        let Some(bundle_id) = bundle_id else {
            return Err(CaptureError::Backend(
                "Asking again works only in the installed app, not when it runs from a terminal".into(),
            ));
        };
        let reset = Command::new("/usr/bin/tccutil")
            .args(["reset", "AudioCapture", &bundle_id])
            .output()
            .map_err(|e| CaptureError::Backend(format!("tccutil: {e}")))?;
        if !reset.status.success() {
            return Err(CaptureError::Backend(format!(
                "tccutil reset AudioCapture {bundle_id}: {}",
                String::from_utf8_lossy(&reset.stderr).trim()
            )));
        }
        request();
        Ok(())
    }

    pub(crate) fn open_settings() -> Result<()> {
        major()?;
        let opened = Command::new("/usr/bin/open")
            .arg(SETTINGS)
            .status()
            .map_err(|e| CaptureError::Backend(format!("open: {e}")))?;
        if opened.success() {
            Ok(())
        } else {
            Err(CaptureError::Backend(format!("open {SETTINGS}: {opened}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_values_map_to_a_status() {
        assert_eq!(from_preflight(Some(0)), PermissionStatus::Allowed);
        assert_eq!(from_preflight(Some(1)), PermissionStatus::Denied);
        assert_eq!(from_preflight(Some(2)), PermissionStatus::NotAsked);
        // Without the function there is no way to tell: capture goes ahead.
        assert_eq!(from_preflight(None), PermissionStatus::Allowed);
    }

    #[test]
    fn settings_have_the_switch_from_macos_15_on() {
        assert_eq!(
            help_for(14),
            PermissionHelp {
                can_open_settings: false,
                can_ask_again: true
            }
        );
        for major in [15, 26] {
            assert_eq!(
                help_for(major),
                PermissionHelp {
                    can_open_settings: true,
                    can_ask_again: true
                }
            );
        }
    }

    #[test]
    fn a_capture_starts_only_when_allowed() {
        let mut asked = 0;
        assert!(start_error(PermissionStatus::Allowed, 15, || asked += 1).is_ok());
        assert_eq!(asked, 0);

        match start_error(PermissionStatus::NotAsked, 15, || asked += 1) {
            Err(CaptureError::Backend(why)) => assert_eq!(why, WAITING),
            other => panic!("{other:?}"),
        }
        assert_eq!(asked, 1, "macOS is asked to show its question");

        match start_error(PermissionStatus::Denied, 15, || asked += 1) {
            Err(CaptureError::PermissionDenied(how)) => assert!(how.contains("Screen & System Audio Recording")),
            other => panic!("{other:?}"),
        }
        match start_error(PermissionStatus::Denied, 14, || asked += 1) {
            Err(CaptureError::PermissionDenied(how)) => assert!(how.contains("Ask again")),
            other => panic!("{other:?}"),
        }
        assert_eq!(asked, 1, "a denial isn't asked again by itself");
    }
}
