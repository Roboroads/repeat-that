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

#[cfg(target_os = "linux")]
pub use portal::PortalAutostart;

/// Starting with the system from inside a Flatpak. The autostart plugin writes `~/.config/autostart/*.desktop`, which in
/// a sandbox is the sandbox's own config folder, so the desktop would never see it. The Background portal makes the
/// entry on the host instead (`flatpak run --command=repeat-that me.roboroads.RepeatThat --hidden`), after the desktop
/// asked the user. It can't tell whether an entry exists, so the stored flag is the truth; see [`reconcile`].
#[cfg(target_os = "linux")]
mod portal {
    use std::sync::mpsc;
    use std::time::Duration;

    use ashpd::desktop::ResponseError;
    use ashpd::desktop::background::{BackgroundProxy, BackgroundRequestOptions};

    use super::Autostart;
    use crate::window::HIDDEN_ARG;

    /// How long to wait for the portal's answer. The desktop asks the user the first time, and a dialog can stay open.
    const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

    /// What the desktop shows in its question.
    const REASON: &str = "Start Repeat That when you log in, so the hotkeys work without opening it.";

    /// The program and arguments of the entry the portal makes: the app, hidden in the tray as in the other packages.
    const COMMAND: [&str; 2] = ["repeat-that", HIDDEN_ARG];

    /// [`Autostart`] on the Background portal.
    pub struct PortalAutostart;

    impl Autostart for PortalAutostart {
        fn is_enabled(&self) -> Result<bool, String> {
            Err("the Background portal can't tell whether Repeat That starts at login".into())
        }

        fn set_enabled(&self, enabled: bool) -> Result<(), String> {
            let (done, answer) = mpsc::channel();
            tauri::async_runtime::spawn(async move {
                let result = match zbus::Connection::session().await {
                    Ok(connection) => request(connection, enabled).await,
                    Err(err) => Err(format!("could not reach the session bus: {err}")),
                };
                let _ = done.send(result);
            });
            answer
                .recv_timeout(ANSWER_TIMEOUT)
                .unwrap_or_else(|_| Err("the system did not answer in time".into()))
        }
    }

    /// Asks the portal on `connection` to start the app with the system (`enabled`) or not. Done when the portal has
    /// answered, which is when the user has.
    pub(super) async fn request(connection: zbus::Connection, enabled: bool) -> Result<(), String> {
        let portal = BackgroundProxy::with_connection(connection).await.map_err(describe)?;
        let options = BackgroundRequestOptions::default()
            .set_reason(REASON)
            .set_auto_start(enabled)
            .set_dbus_activatable(false)
            .set_command(COMMAND);
        let granted = portal
            .request_background(None, options)
            .await
            .map_err(describe)?
            .response()
            .map_err(describe)?;
        match (enabled, granted.auto_start()) {
            (true, false) => Err("the system did not allow Repeat That to start at login".into()),
            (false, true) => Err("the system still starts Repeat That at login".into()),
            _ => Ok(()),
        }
    }

    fn describe(err: ashpd::Error) -> String {
        match err {
            ashpd::Error::Response(ResponseError::Cancelled) => "the question was closed without an answer".into(),
            ashpd::Error::PortalNotFound(_) => "the desktop portal is not running".into(),
            err => err.to_string(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::autostart::reconcile;
        use crate::fake_portal::{BackgroundAnswer, BackgroundCall, FakePortal};
        use crate::private_bus::PrivateBus;

        /// A fake portal on a bus of its own, and a connection to it. `None` (the test skips) without `dbus-daemon`.
        struct Rig {
            _bus: PrivateBus,
            portal: FakePortal,
            connection: zbus::Connection,
        }

        fn rig(with_background: bool) -> Option<Rig> {
            let bus = PrivateBus::start()?;
            let portal = FakePortal::start_without_shortcuts(&bus);
            if with_background {
                portal.add_background();
            }
            let connection = zbus::block_on(async {
                zbus::connection::Builder::address(bus.address.as_str())
                    .unwrap()
                    .build()
                    .await
                    .unwrap()
            });
            Some(Rig {
                _bus: bus,
                portal,
                connection,
            })
        }

        fn calls(rig: &Rig) -> Vec<BackgroundCall> {
            rig.portal.state.backgrounds.lock().unwrap().clone()
        }

        #[test]
        fn turning_it_on_asks_for_a_hidden_start_at_login() {
            let Some(rig) = rig(true) else { return };
            assert_eq!(zbus::block_on(request(rig.connection.clone(), true)), Ok(()));
            let calls = calls(&rig);
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].autostart, Some(true));
            assert_eq!(calls[0].command, ["repeat-that", "--hidden"]);
            assert_eq!(calls[0].dbus_activatable, Some(false));
            assert!(calls[0].reason.as_deref().is_some_and(|reason| !reason.is_empty()));
        }

        #[test]
        fn turning_it_off_asks_for_no_start_at_login() {
            let Some(rig) = rig(true) else { return };
            assert_eq!(zbus::block_on(request(rig.connection.clone(), false)), Ok(()));
            assert_eq!(calls(&rig)[0].autostart, Some(false));
        }

        #[test]
        fn a_no_from_the_user_is_an_error_when_turning_it_on() {
            let Some(rig) = rig(true) else { return };
            rig.portal.answer_background_with(BackgroundAnswer::Deny);
            let err = zbus::block_on(request(rig.connection.clone(), true)).unwrap_err();
            assert!(err.contains("did not allow"), "{err}");
        }

        #[test]
        fn a_no_from_the_user_is_fine_when_turning_it_off() {
            let Some(rig) = rig(true) else { return };
            rig.portal.answer_background_with(BackgroundAnswer::Deny);
            assert_eq!(zbus::block_on(request(rig.connection.clone(), false)), Ok(()));
        }

        #[test]
        fn a_closed_question_is_an_error() {
            let Some(rig) = rig(true) else { return };
            rig.portal.answer_background_with(BackgroundAnswer::Dismiss);
            let err = zbus::block_on(request(rig.connection.clone(), true)).unwrap_err();
            assert!(err.contains("closed"), "{err}");
        }

        #[test]
        fn a_portal_that_fails_is_an_error() {
            let Some(rig) = rig(true) else { return };
            rig.portal.answer_background_with(BackgroundAnswer::Fail);
            assert!(zbus::block_on(request(rig.connection.clone(), true)).is_err());
        }

        #[test]
        fn a_desktop_without_the_background_portal_is_an_error() {
            let Some(rig) = rig(false) else { return };
            assert!(zbus::block_on(request(rig.connection.clone(), true)).is_err());
            assert!(calls(&rig).is_empty());
        }

        #[test]
        fn the_portal_cannot_report_so_the_stored_flag_stays() {
            let state = PortalAutostart.is_enabled();
            assert!(state.is_err());
            assert!(reconcile(true, state.clone()));
            assert!(!reconcile(false, state));
        }
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
