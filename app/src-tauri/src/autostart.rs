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
///
/// The portal answers when the user does, which can take minutes, so nothing here waits for it:
/// [`set_enabled`](Autostart::set_enabled) sends the request and returns, with the settings window and everything else
/// still running. The answer comes later and is checked against what the settings want by then
/// ([`Inner::settled`]): a refusal turns the setting back off, and a late "yes" to what has been turned off since is
/// taken back.
///
/// The app is also a background app, one with no window while it sits in the tray, and a desktop may end such an app
/// that has not been allowed to run in the background. [`PortalAutostart::ask_for_background`] asks for that at start.
/// The portal remembers the answer, so it asks the user once. If the answer is no, the app says so on its error output
/// and the documentation says how to allow it (`docs/install-linux.md`); it doesn't change what the app does.
#[cfg(target_os = "linux")]
mod portal {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use ashpd::desktop::ResponseError;
    use ashpd::desktop::background::{BackgroundProxy, BackgroundRequestOptions};

    use super::Autostart;
    use crate::locking::lock;
    use crate::window::HIDDEN_ARG;

    /// What the desktop shows in its question.
    const REASON: &str = "Start Repeat That when you log in, so the hotkeys work without opening it.";

    /// The program and arguments of the entry the portal makes: the app, hidden in the tray as in the other packages.
    const COMMAND: [&str; 2] = ["repeat-that", HIDDEN_ARG];

    /// What the app says when the system won't let it run in the background.
    const BACKGROUND_REFUSED: &str = "the system does not allow Repeat That to run in the background, so it may end it \
        once its window is closed. Allow it in your desktop's settings for the app, or with: \
        flatpak permission-set background background me.roboroads.RepeatThat yes";

    /// What the portal said to a request.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct Granted {
        /// The app may run in the background.
        pub background: bool,
        /// The app is started when the user logs in.
        pub autostart: bool,
    }

    type Answer = Pin<Box<dyn Future<Output = Result<Granted, String>> + Send>>;
    /// Sends a request to the portal: `Some(true)` for starting at login, `Some(false)` for not, `None` for the
    /// permission to run in the background alone.
    type Ask = dyn Fn(Option<bool>) -> Answer + Send + Sync;
    /// Called, from a thread of its own, with why the system won't start the app at login after all.
    type Refused = dyn Fn(String) + Send + Sync;

    /// [`Autostart`] on the Background portal. A refusal is handed to `refused`, which turns the setting off again.
    pub struct PortalAutostart {
        inner: Arc<Inner>,
    }

    struct Inner {
        ask: Box<Ask>,
        refused: Box<Refused>,
        /// Whether the settings want the app to start at login, as of the last [`set_enabled`](Autostart::set_enabled).
        wanted: Mutex<bool>,
    }

    impl PortalAutostart {
        pub fn new(refused: impl Fn(String) + Send + Sync + 'static) -> Self {
            Self::with(Box::new(ask_over_the_session_bus), Box::new(refused))
        }

        fn with(ask: Box<Ask>, refused: Box<Refused>) -> Self {
            PortalAutostart {
                inner: Arc::new(Inner {
                    ask,
                    refused,
                    wanted: Mutex::new(false),
                }),
            }
        }

        /// Asks for the permission to run in the background, without touching Start at login. The desktop asks the
        /// user the first time, and the portal gives the same answer unasked after that. Returns at once.
        pub fn ask_for_background(&self) {
            let inner = Arc::clone(&self.inner);
            tauri::async_runtime::spawn(async move {
                match (inner.ask)(None).await {
                    Ok(granted) => {
                        if let Some(note) = background_note(granted) {
                            eprintln!("{note}");
                        }
                    }
                    Err(err) => eprintln!("could not ask whether Repeat That may run in the background: {err}"),
                }
            });
        }
    }

    /// What to tell the user about the answer to the permission request, if anything.
    fn background_note(granted: Granted) -> Option<&'static str> {
        (!granted.background).then_some(BACKGROUND_REFUSED)
    }

    impl Autostart for PortalAutostart {
        fn is_enabled(&self) -> Result<bool, String> {
            Err("the Background portal can't tell whether Repeat That starts at login".into())
        }

        fn set_enabled(&self, enabled: bool) -> Result<(), String> {
            *lock(&self.inner.wanted) = enabled;
            self.inner.send(enabled);
            Ok(())
        }
    }

    impl Inner {
        /// Sends the request on a task of the async runtime, and deals with the answer whenever it comes.
        fn send(self: &Arc<Self>, enabled: bool) {
            let inner = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                let answer = (inner.ask)(Some(enabled)).await;
                inner.settled(enabled, answer);
            });
        }

        /// The portal answered the request to turn start at login `asked` on or off. What the settings want now may
        /// be something else, because the user went on while the question was open.
        fn settled(self: &Arc<Self>, asked: bool, answer: Result<Granted, String>) {
            let active = match &answer {
                Ok(granted) => granted.autostart,
                Err(_) => false,
            };
            if asked {
                if active {
                    // Allowed, but turned off since: the entry exists now, and nobody wants it.
                    if !*lock(&self.wanted) {
                        self.send(false);
                    }
                } else {
                    // Refused, or failed. Only a setting that is still on needs to hear of it.
                    let was_wanted = std::mem::replace(&mut *lock(&self.wanted), false);
                    if was_wanted {
                        let why = match answer {
                            Err(err) => err,
                            Ok(_) => "the system did not allow Repeat That to start at login".into(),
                        };
                        (self.refused)(why);
                    }
                }
            } else if active {
                eprintln!("the system still starts Repeat That at login, though it was turned off");
            } else if let Err(err) = answer {
                eprintln!("could not turn off starting Repeat That at login: {err}");
            }
        }
    }

    fn ask_over_the_session_bus(autostart: Option<bool>) -> Answer {
        Box::pin(async move {
            let connection = zbus::Connection::session()
                .await
                .map_err(|err| format!("could not reach the session bus: {err}"))?;
            request(connection, autostart).await
        })
    }

    /// Sends one request over `connection` and waits for the answer, which is when the user has answered.
    pub(super) async fn request(connection: zbus::Connection, autostart: Option<bool>) -> Result<Granted, String> {
        let portal = BackgroundProxy::with_connection(connection).await.map_err(describe)?;
        let mut options = BackgroundRequestOptions::default()
            .set_reason(REASON)
            .set_auto_start(autostart)
            .set_dbus_activatable(false);
        if autostart.is_some() {
            options = options.set_command(COMMAND);
        }
        let answer = portal
            .request_background(None, options)
            .await
            .map_err(describe)?
            .response()
            .map_err(describe)?;
        Ok(Granted {
            background: answer.run_in_background(),
            autostart: answer.auto_start(),
        })
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
        use std::time::{Duration, Instant};

        use tauri::async_runtime::Sender;

        use super::*;
        use crate::autostart::reconcile;
        use crate::fake_portal::{BackgroundAnswer, BackgroundCall, FakePortal};
        use crate::private_bus::PrivateBus;

        // The wire: the requests as the portal receives them.

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

        fn granted(background: bool, autostart: bool) -> Granted {
            Granted { background, autostart }
        }

        #[test]
        fn turning_it_on_asks_for_a_hidden_start_at_login() {
            let Some(rig) = rig(true) else { return };
            assert_eq!(
                zbus::block_on(request(rig.connection.clone(), Some(true))),
                Ok(granted(true, true))
            );
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
            assert_eq!(
                zbus::block_on(request(rig.connection.clone(), Some(false))),
                Ok(granted(true, false))
            );
            assert_eq!(calls(&rig)[0].autostart, Some(false));
        }

        #[test]
        fn asking_for_the_background_alone_leaves_start_at_login_unset() {
            let Some(rig) = rig(true) else { return };
            assert_eq!(
                zbus::block_on(request(rig.connection.clone(), None)),
                Ok(granted(true, false))
            );
            let calls = calls(&rig);
            assert_eq!(calls[0].autostart, None);
            assert!(calls[0].command.is_empty());
        }

        #[test]
        fn a_no_from_the_user_is_an_answer_not_an_error() {
            let Some(rig) = rig(true) else { return };
            rig.portal.answer_background_with(BackgroundAnswer::Deny);
            assert_eq!(
                zbus::block_on(request(rig.connection.clone(), Some(true))),
                Ok(granted(false, false))
            );
        }

        #[test]
        fn a_closed_question_is_an_error() {
            let Some(rig) = rig(true) else { return };
            rig.portal.answer_background_with(BackgroundAnswer::Dismiss);
            let err = zbus::block_on(request(rig.connection.clone(), Some(true))).unwrap_err();
            assert!(err.contains("closed"), "{err}");
        }

        #[test]
        fn a_portal_that_fails_is_an_error() {
            let Some(rig) = rig(true) else { return };
            rig.portal.answer_background_with(BackgroundAnswer::Fail);
            assert!(zbus::block_on(request(rig.connection.clone(), Some(true))).is_err());
        }

        #[test]
        fn a_desktop_without_the_background_portal_is_an_error() {
            let Some(rig) = rig(false) else { return };
            assert!(zbus::block_on(request(rig.connection.clone(), Some(true))).is_err());
            assert!(calls(&rig).is_empty());
        }

        // What the app makes of the answers.

        type Replies = Arc<Mutex<Vec<Sender<Result<Granted, String>>>>>;

        /// A portal whose answers the test gives, in any order, and what it was asked and told.
        struct Held {
            asked: Arc<Mutex<Vec<Option<bool>>>>,
            replies: Replies,
            refused: Arc<Mutex<Vec<String>>>,
            portal: PortalAutostart,
        }

        fn held() -> Held {
            let asked = Arc::new(Mutex::new(Vec::new()));
            let replies = Arc::new(Mutex::new(Vec::new()));
            let refused = Arc::new(Mutex::new(Vec::new()));
            let (log, senders, told) = (Arc::clone(&asked), Arc::clone(&replies), Arc::clone(&refused));
            let ask: Box<Ask> = Box::new(move |autostart| {
                let (reply, mut waiting) = tauri::async_runtime::channel(1);
                log.lock().unwrap().push(autostart);
                senders.lock().unwrap().push(reply);
                Box::pin(async move { waiting.recv().await.unwrap_or_else(|| Err("no answer".into())) })
            });
            let portal = PortalAutostart::with(ask, Box::new(move |why| told.lock().unwrap().push(why)));
            Held {
                asked,
                replies,
                refused,
                portal,
            }
        }

        impl Held {
            fn asked(&self) -> Vec<Option<bool>> {
                self.asked.lock().unwrap().clone()
            }

            /// Answers the `index`th request.
            fn answer(&self, index: usize, answer: Result<Granted, String>) {
                self.replies.lock().unwrap()[index]
                    .try_send(answer)
                    .expect("a request waiting");
            }

            fn refused(&self) -> Vec<String> {
                self.refused.lock().unwrap().clone()
            }

            /// Waits until `condition` holds (the answers are dealt with on the async runtime).
            fn wait_for(&self, what: &str, condition: impl Fn(&Held) -> bool) {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !condition(self) {
                    assert!(Instant::now() < deadline, "timed out waiting for {what}");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }

            /// Gives what is going to happen, if anything, a moment to.
            fn settle(&self) {
                std::thread::sleep(Duration::from_millis(150));
            }
        }

        #[test]
        fn turning_it_on_does_not_wait_for_the_user() {
            let held = held();
            let started = Instant::now();
            assert_eq!(held.portal.set_enabled(true), Ok(()));
            assert!(started.elapsed() < Duration::from_secs(1), "waited for the answer");
            held.wait_for("the request", |held| held.asked() == [Some(true)]);
            assert!(held.refused().is_empty());
        }

        #[test]
        fn a_yes_changes_nothing_else() {
            let held = held();
            held.portal.set_enabled(true).unwrap();
            held.wait_for("the request", |held| held.asked().len() == 1);
            held.answer(0, Ok(granted(true, true)));
            held.settle();
            assert_eq!(held.asked(), [Some(true)]);
            assert!(held.refused().is_empty());
        }

        #[test]
        fn a_no_turns_the_setting_back_off() {
            let held = held();
            held.portal.set_enabled(true).unwrap();
            held.wait_for("the request", |held| held.asked().len() == 1);
            held.answer(0, Ok(granted(false, false)));
            held.wait_for("the refusal", |held| held.refused().len() == 1);
            assert!(held.refused()[0].contains("did not allow"), "{:?}", held.refused());
            assert_eq!(held.asked(), [Some(true)]);
        }

        #[test]
        fn a_request_that_fails_turns_the_setting_back_off() {
            let held = held();
            held.portal.set_enabled(true).unwrap();
            held.wait_for("the request", |held| held.asked().len() == 1);
            held.answer(0, Err("the portal is gone".into()));
            held.wait_for("the refusal", |held| held.refused() == ["the portal is gone"]);
        }

        #[test]
        fn a_late_yes_to_what_was_turned_off_since_is_taken_back() {
            let held = held();
            held.portal.set_enabled(true).unwrap();
            held.wait_for("the first request", |held| held.asked().len() == 1);
            held.portal.set_enabled(false).unwrap();
            held.wait_for("the second request", |held| held.asked().len() == 2);
            held.answer(1, Ok(granted(true, false)));
            // The question for the first request was still open, and the user says yes to it now.
            held.answer(0, Ok(granted(true, true)));
            held.wait_for("the correction", |held| held.asked().len() == 3);
            assert_eq!(held.asked(), [Some(true), Some(false), Some(false)]);
            assert!(held.refused().is_empty());
        }

        #[test]
        fn a_late_no_to_what_was_turned_off_since_is_not_a_refusal() {
            let held = held();
            held.portal.set_enabled(true).unwrap();
            held.wait_for("the first request", |held| held.asked().len() == 1);
            held.portal.set_enabled(false).unwrap();
            held.wait_for("the second request", |held| held.asked().len() == 2);
            held.answer(0, Ok(granted(false, false)));
            held.settle();
            assert!(held.refused().is_empty());
            assert_eq!(held.asked().len(), 2);
        }

        #[test]
        fn turning_it_off_never_reports_a_refusal() {
            let held = held();
            held.portal.set_enabled(false).unwrap();
            held.wait_for("the request", |held| held.asked().len() == 1);
            held.answer(0, Err("the portal is gone".into()));
            held.settle();
            assert!(held.refused().is_empty());
        }

        #[test]
        fn the_background_permission_is_asked_alone_and_a_no_is_not_a_refusal_of_start_at_login() {
            let held = held();
            held.portal.ask_for_background();
            held.wait_for("the request", |held| held.asked() == [None]);
            held.answer(0, Ok(granted(false, false)));
            held.settle();
            assert!(held.refused().is_empty());
            assert_eq!(held.asked(), [None]);
        }

        #[test]
        fn a_background_no_is_noted_with_how_to_allow_it() {
            let note = background_note(granted(false, false)).expect("a note");
            assert!(note.contains("flatpak permission-set background background me.roboroads.RepeatThat yes"));
            assert_eq!(background_note(granted(true, false)), None);
        }

        #[test]
        fn the_portal_cannot_report_so_the_stored_flag_stays() {
            let portal = PortalAutostart::new(|_| {});
            let state = portal.is_enabled();
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
