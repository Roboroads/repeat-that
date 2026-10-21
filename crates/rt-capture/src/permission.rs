//! Whether the OS lets this app capture other apps' audio, and what the user can do about it from inside the app.
//!
//! Only macOS asks the user for this (System Settings > Privacy & Security > Screen & System Audio Recording). Without
//! it, starting a capture fails with [`crate::CaptureError::PermissionDenied`]. Elsewhere every function here returns
//! [`crate::CaptureError::Unsupported`], so a caller can tell "nothing to fix here" from a real failure.

#[cfg(not(target_os = "macos"))]
use crate::CaptureError;
use crate::Result;

/// Whether this app may capture audio, as far as the OS says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    Allowed,
    /// The user said no, or took it back.
    Denied,
    /// The user hasn't been asked yet. Starting a capture asks.
    NotAsked,
}

/// What the user can do about a missing permission from inside the app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PermissionHelp {
    /// [`open_settings`] opens the page of the OS settings where the permission is given.
    pub can_open_settings: bool,
    /// [`ask_again`] makes the OS ask the user once more.
    pub can_ask_again: bool,
}

/// Whether this app may capture audio right now.
pub fn status() -> Result<PermissionStatus> {
    #[cfg(target_os = "macos")]
    return crate::macos::permission::status();
    #[cfg(not(target_os = "macos"))]
    Err(unsupported())
}

/// Which of [`open_settings`] and [`ask_again`] work on this OS version.
pub fn help() -> Result<PermissionHelp> {
    #[cfg(target_os = "macos")]
    return crate::macos::permission::help();
    #[cfg(not(target_os = "macos"))]
    Err(unsupported())
}

/// Forgets the user's earlier answer and asks again. Returns once the OS shows its question, not when the user has
/// answered; a capture that is retried starts once they allow it. Only works in the installed app, which macOS knows
/// by its bundle ID.
pub fn ask_again() -> Result<()> {
    #[cfg(target_os = "macos")]
    return crate::macos::permission::ask_again();
    #[cfg(not(target_os = "macos"))]
    Err(unsupported())
}

/// Opens the OS settings where the user gives the permission.
pub fn open_settings() -> Result<()> {
    #[cfg(target_os = "macos")]
    return crate::macos::permission::open_settings();
    #[cfg(not(target_os = "macos"))]
    Err(unsupported())
}

#[cfg(not(target_os = "macos"))]
fn unsupported() -> CaptureError {
    CaptureError::Unsupported(format!(
        "{} doesn't ask for permission to capture audio",
        std::env::consts::OS
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_systems_have_nothing_to_allow() {
        use crate::CaptureError::Unsupported;

        assert!(matches!(status(), Err(Unsupported(_))));
        assert!(matches!(help(), Err(Unsupported(_))));
        assert!(matches!(ask_again(), Err(Unsupported(_))));
        assert!(matches!(open_settings(), Err(Unsupported(_))));
        match help() {
            Err(Unsupported(why)) => assert!(why.contains(std::env::consts::OS), "{why}"),
            _ => unreachable!(),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_always_has_a_way_to_ask_again() {
        // CI runs macOS 14.2 or later, where asking again always works; opening the settings needs 15.
        let help = help().unwrap();
        assert!(help.can_ask_again);
        assert!(status().is_ok());
    }
}
