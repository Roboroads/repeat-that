//! The backend of an OS without capture support. It has the items every OS module has (see the top of `lib.rs`) and
//! refuses all of them.

use std::path::Path;

use crate::{AppAudioSource, CaptureError, IconImage, PlatformOptions, Result};

pub(crate) const HAS_BACKEND: bool = false;
pub(crate) const HAS_DUCK_GUARD: bool = false;
pub(crate) const EXE_PATHS_IGNORE_CASE: bool = false;

pub(crate) fn source(_options: PlatformOptions) -> Result<Box<dyn AppAudioSource>> {
    Err(CaptureError::Unsupported(format!(
        "no capture backend for {} yet",
        std::env::consts::OS
    )))
}

pub(crate) fn icon(_exe: &Path, _size_px: u32) -> Option<IconImage> {
    None
}

/// There is no guard here (`HAS_DUCK_GUARD` is false), so nothing ever starts one: `false` leaves a request for one to
/// the app.
pub(crate) fn run_duck_guard(_journal: &Path, _parent: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_has_no_backend() {
        const { assert!(!HAS_BACKEND) };
        const { assert!(!HAS_DUCK_GUARD) };
        const { assert!(!crate::HAS_PLATFORM_BACKEND) };
    }

    #[test]
    fn its_source_is_unsupported_and_names_the_os() {
        match source(PlatformOptions::default()) {
            Err(CaptureError::Unsupported(why)) => assert!(why.contains(std::env::consts::OS), "{why}"),
            _ => panic!("expected Unsupported"),
        }
    }

    #[test]
    fn it_has_no_icons_and_no_guard() {
        assert_eq!(icon(Path::new("app"), 32), None);
        assert!(!run_duck_guard(Path::new("duck.journal"), 1234));
    }
}
