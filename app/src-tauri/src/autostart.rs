//! Starting with the OS. The OS owns the entry (the Run key on Windows), so it wins over the stored flag.

use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt;

/// Reads and writes the OS autostart entry. The real one is the autostart plugin; tests use a fake.
pub trait Autostart: Send + Sync {
    fn is_enabled(&self) -> Result<bool, String>;
    fn set_enabled(&self, enabled: bool) -> Result<(), String>;
}

/// [`Autostart`] on `tauri-plugin-autostart`.
pub struct OsAutostart<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> Autostart for OsAutostart<R> {
    fn is_enabled(&self) -> Result<bool, String> {
        self.0.autolaunch().is_enabled().map_err(|err| err.to_string())
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let launcher = self.0.autolaunch();
        if enabled { launcher.enable() } else { launcher.disable() }.map_err(|err| err.to_string())
    }
}

/// The autostart flag to use at startup: what the OS says, so an entry removed outside the app (or added by hand)
/// shows correctly. When the OS can't be asked the stored flag stays.
pub fn reconcile(stored: bool, os: Result<bool, String>) -> bool {
    os.unwrap_or(stored)
}

#[cfg(test)]
pub mod fake {
    use std::sync::Mutex;

    use super::Autostart;

    /// An autostart entry in memory that can be told to fail.
    #[derive(Default)]
    pub struct FakeAutostart {
        pub enabled: Mutex<bool>,
        pub fail_reads: Mutex<bool>,
        pub fail_writes: Mutex<bool>,
        /// Every `set_enabled` call, failed ones too.
        pub writes: Mutex<Vec<bool>>,
    }

    impl FakeAutostart {
        pub fn with_enabled(enabled: bool) -> Self {
            let fake = FakeAutostart::default();
            *fake.enabled.lock().unwrap() = enabled;
            fake
        }
    }

    impl Autostart for FakeAutostart {
        fn is_enabled(&self) -> Result<bool, String> {
            if *self.fail_reads.lock().unwrap() {
                return Err("registry unreadable".into());
            }
            Ok(*self.enabled.lock().unwrap())
        }

        fn set_enabled(&self, enabled: bool) -> Result<(), String> {
            self.writes.lock().unwrap().push(enabled);
            if *self.fail_writes.lock().unwrap() {
                return Err("registry is read-only".into());
            }
            *self.enabled.lock().unwrap() = enabled;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_os_state_overrides_the_stored_flag() {
        assert!(reconcile(false, Ok(true)), "entry added outside the app");
        assert!(!reconcile(true, Ok(false)), "entry removed outside the app");
        assert!(reconcile(true, Ok(true)));
        assert!(!reconcile(false, Ok(false)));
    }

    #[test]
    fn the_stored_flag_stays_when_the_os_cannot_be_asked() {
        assert!(reconcile(true, Err("denied".into())));
        assert!(!reconcile(false, Err("denied".into())));
    }
}
