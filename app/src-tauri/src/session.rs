//! The desktop the app runs on: which window system, whether the desktop's shortcut portal answers, whether a tray host
//! is there, and the command that starts this install of the app. Only Linux has more than one answer to these; on
//! Windows and macOS the hotkeys always go through the plugin and the tray is always there.
//!
//! Everything that asks the system sits behind [`Env`] and [`DesktopBus`], so the rules can be tested without a
//! session bus.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ipc::{HotkeyBackendKind, HotkeyCommands};
use crate::settings::HOTKEYS;

/// The window system the app runs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayServer {
    Wayland,
    X11,
    /// A Linux session with no display at all (a login over ssh, a service).
    NoDisplay,
    /// Not Linux: Windows or macOS, where the OS has no such split.
    Native,
}

/// Environment variables, so tests don't have to touch the process's own.
pub trait Env {
    /// The value of `name`; `None` when it is unset or empty.
    fn var(&self, name: &str) -> Option<String>;
}

/// The process's own environment.
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }
}

/// The window system of a Linux session. Wayland wins when both are set: that is XWayland, where the hotkeys of X11
/// would only fire while an X11 window has focus.
pub fn display_server(env: &dyn Env) -> DisplayServer {
    if env.var("WAYLAND_DISPLAY").is_some() || env.var("XDG_SESSION_TYPE").as_deref() == Some("wayland") {
        DisplayServer::Wayland
    } else if env.var("DISPLAY").is_some() {
        DisplayServer::X11
    } else {
        DisplayServer::NoDisplay
    }
}

/// [`display_server`] on Linux, [`DisplayServer::Native`] elsewhere.
pub fn os_display(env: &dyn Env) -> DisplayServer {
    if cfg!(target_os = "linux") {
        display_server(env)
    } else {
        DisplayServer::Native
    }
}

/// What hears of the tray host coming and going: whether there is one now.
pub type TrayHostListener = Box<dyn Fn(bool) + Send>;

/// The session bus, as far as the app asks it questions. The real one is [`system_bus`]; tests use a fake.
pub trait DesktopBus: Send + Sync {
    /// The version of the GlobalShortcuts portal, if the portal answers.
    fn global_shortcuts_version(&self) -> Option<u32>;
    /// Whether something owns `org.kde.StatusNotifierWatcher`, which a tray icon needs.
    fn tray_host_present(&self) -> bool;
    /// Calls `on_change` with the new answer of [`tray_host_present`](Self::tray_host_present) whenever the watcher
    /// appears or goes away. Returns at once; the calls come from a thread of the bus's own.
    fn watch_tray_host(&self, on_change: TrayHostListener);
}

/// What the session offers the app for hotkeys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    pub display: DisplayServer,
    /// The version of the GlobalShortcuts portal; `None` when the portal isn't there (or isn't asked, see [`detect`]).
    pub portal_version: Option<u32>,
}

/// Looks at the session. The portal is only asked on Wayland: that is the only place it is used, and asking costs a
/// round trip that may have to start the portal service.
pub fn detect(env: &dyn Env, bus: &dyn DesktopBus) -> Session {
    detect_for(os_display(env), bus)
}

/// [`detect`] for a session that is known to run on `display`.
fn detect_for(display: DisplayServer, bus: &dyn DesktopBus) -> Session {
    let portal_version = if display == DisplayServer::Wayland {
        bus.global_shortcuts_version()
    } else {
        None
    };
    Session {
        display,
        portal_version,
    }
}

/// How the hotkeys work in `session`: the plugin where it works (Windows, macOS, X11), the portal on a Wayland desktop
/// that has one, and nothing otherwise.
pub fn choose_backend(session: &Session) -> HotkeyBackendKind {
    match (session.display, session.portal_version) {
        (DisplayServer::Native | DisplayServer::X11, _) => HotkeyBackendKind::Plugin,
        (DisplayServer::Wayland, Some(_)) => HotkeyBackendKind::Portal,
        (DisplayServer::Wayland, None) | (DisplayServer::NoDisplay, _) => HotkeyBackendKind::None,
    }
}

/// Whether the global-shortcut plugin is wanted at all, which has to be known before the app is built. Initialising it
/// without an X server (Wayland, no display) would only fail.
pub fn wants_shortcut_plugin(env: &dyn Env) -> bool {
    matches!(os_display(env), DisplayServer::Native | DisplayServer::X11)
}

/// What the app found out about the desktop, kept for the whole run (the tray host can come and go).
pub struct SessionState {
    session: Session,
    tray_available: AtomicBool,
}

impl SessionState {
    pub fn new(session: Session, tray_available: bool) -> Self {
        SessionState {
            session,
            tray_available: AtomicBool::new(tray_available),
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn tray_available(&self) -> bool {
        self.tray_available.load(Ordering::SeqCst)
    }

    /// Remembers whether a tray host is there. Returns whether that is news.
    pub fn set_tray_available(&self, available: bool) -> bool {
        self.tray_available.swap(available, Ordering::SeqCst) != available
    }
}

/// Looks at the desktop, and has `on_tray_change` called whenever the tray host shows up or goes away later (enabling
/// the AppIndicator extension on GNOME, say), so the tray starts working without a restart.
pub fn start(env: &dyn Env, bus: &dyn DesktopBus, on_tray_change: impl Fn() + Send + 'static) -> Arc<SessionState> {
    let state = Arc::new(SessionState::new(detect(env, bus), bus.tray_host_present()));
    let watched = Arc::clone(&state);
    bus.watch_tray_host(Box::new(move |present| {
        if watched.set_tray_available(present) {
            on_tray_change();
        }
    }));
    state
}

/// The name Flatpak gives the app, which `flatpak run` takes.
const FLATPAK_ID: &str = "FLATPAK_ID";
/// The path of the AppImage file that is running.
const APPIMAGE: &str = "APPIMAGE";
/// What a package installs the program as.
const BINARY: &str = "repeat-that";

/// The command that starts this install of the app, which is what the command-line actions are appended to. A Flatpak
/// runs through `flatpak run`, an AppImage is its own file, and everything else is on the `PATH`.
pub fn launch_command(env: &dyn Env) -> String {
    if let Some(id) = env.var(FLATPAK_ID) {
        format!("flatpak run {id}")
    } else if let Some(path) = env.var(APPIMAGE) {
        quoted(&path)
    } else {
        BINARY.to_string()
    }
}

/// `text` in double quotes, for a command line that a shell, or a desktop's shortcut settings, will split into words.
fn quoted(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// The command that runs each hotkey action in the running app.
pub fn hotkey_commands(env: &dyn Env) -> HotkeyCommands {
    let launch = launch_command(env);
    let mut commands = HotkeyCommands {
        replay: String::new(),
        stop: String::new(),
        catch_up: String::new(),
    };
    for (slot, def) in commands.slots_mut().into_iter().zip(&HOTKEYS) {
        *slot = format!("{launch} --{}", def.command);
    }
    commands
}

/// The session bus of this desktop.
#[cfg(target_os = "linux")]
pub fn system_bus() -> Arc<dyn DesktopBus> {
    Arc::new(linux::SessionBus::default())
}

/// Not Linux: no portal and a tray that is always there.
#[cfg(not(target_os = "linux"))]
pub fn system_bus() -> Arc<dyn DesktopBus> {
    Arc::new(NativeBus)
}

#[cfg(not(target_os = "linux"))]
struct NativeBus;

#[cfg(not(target_os = "linux"))]
impl DesktopBus for NativeBus {
    fn global_shortcuts_version(&self) -> Option<u32> {
        None
    }

    fn tray_host_present(&self) -> bool {
        true
    }

    fn watch_tray_host(&self, _on_change: TrayHostListener) {}
}

/// The session state of the running app, managed by Tauri.
pub type SharedSession = Arc<SessionState>;

/// Looks at the desktop and manages the result as [`SharedSession`]; the tray host turning up or going away is
/// published to the window and the tray. Needs nothing else from the app, and everything that chooses how the hotkeys
/// work asks for it, so it comes first.
pub fn install<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let handle = app.clone();
    let state = start(&ProcessEnv, &*system_bus(), move || {
        crate::engine::publish_status(&handle)
    });
    tauri::Manager::manage(app, state);
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::mpsc;
    use std::time::Duration;

    use zbus::blocking::{
        Connection,
        connection::Builder,
        fdo::{DBusProxy, PropertiesProxy},
    };
    use zbus::names::InterfaceName;

    use super::{DesktopBus, TrayHostListener};

    /// Where the portals live, and the interface the hotkeys use.
    const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
    const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
    const SHORTCUTS_INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

    /// What owns the tray: whoever holds this name can show a tray icon (KDE's, GNOME's AppIndicator extension, and
    /// the other desktops' hosts all answer to it).
    const TRAY_WATCHER: &str = "org.kde.StatusNotifierWatcher";

    /// How long to wait for the portal to answer. It is started by the first call and normally answers within a
    /// fraction of a second; a portal that doesn't is treated as not there, which shows the warning with the commands.
    const PORTAL_TIMEOUT: Duration = Duration::from_secs(3);

    /// The user's session bus, or any bus at `address`, which is for tests.
    #[derive(Default)]
    pub struct SessionBus {
        address: Option<String>,
    }

    impl SessionBus {
        /// A bus at `address` (`unix:path=…`) instead of the session bus.
        #[cfg(test)]
        pub fn at(address: &str) -> Self {
            SessionBus {
                address: Some(address.to_string()),
            }
        }

        fn connect(&self) -> zbus::Result<Connection> {
            match &self.address {
                Some(address) => Builder::address(address.as_str())?.build(),
                None => Connection::session(),
            }
        }

        /// The `version` the portal's GlobalShortcuts interface reports. Asked of the interface itself: a portal that is
        /// there but has no backend for shortcuts (GNOME before 48) answers for its other interfaces and fails for this
        /// one, and a library's idea of "the portal is there" would wrongly take it for one that has shortcuts.
        fn read_portal_version(&self) -> Result<u32, String> {
            let connection = self.connect().map_err(|err| err.to_string())?;
            let properties = PropertiesProxy::builder(&connection)
                .destination(PORTAL_NAME)
                .and_then(|builder| builder.path(PORTAL_PATH))
                .and_then(|builder| builder.build())
                .map_err(|err| err.to_string())?;
            let interface = InterfaceName::try_from(SHORTCUTS_INTERFACE).map_err(|err| err.to_string())?;
            let version = properties.get(interface, "version").map_err(|err| err.to_string())?;
            u32::try_from(version).map_err(|err| err.to_string())
        }

        fn watcher_has_owner(dbus: &DBusProxy<'_>) -> bool {
            match zbus::names::BusName::try_from(TRAY_WATCHER) {
                Ok(name) => dbus.name_has_owner(name).unwrap_or(false),
                Err(_) => false,
            }
        }
    }

    impl DesktopBus for SessionBus {
        fn global_shortcuts_version(&self) -> Option<u32> {
            let bus = SessionBus {
                address: self.address.clone(),
            };
            let (answer, asked) = mpsc::channel();
            // A thread of its own, so a portal that hangs can't hold the start of the app: the wait below ends.
            let spawned = std::thread::Builder::new()
                .name("rt-portal-probe".into())
                .spawn(move || {
                    let _ = answer.send(bus.read_portal_version());
                });
            if spawned.is_err() {
                return None;
            }
            match asked.recv_timeout(PORTAL_TIMEOUT) {
                Ok(Ok(version)) => Some(version),
                Ok(Err(err)) => {
                    eprintln!("no shortcuts portal: {err}");
                    None
                }
                Err(_) => {
                    eprintln!("the shortcuts portal did not answer within {PORTAL_TIMEOUT:?}");
                    None
                }
            }
        }

        fn tray_host_present(&self) -> bool {
            let Ok(connection) = self.connect() else { return false };
            let Ok(dbus) = DBusProxy::new(&connection) else {
                return false;
            };
            Self::watcher_has_owner(&dbus)
        }

        fn watch_tray_host(&self, on_change: TrayHostListener) {
            let bus = SessionBus {
                address: self.address.clone(),
            };
            let watch = move || -> zbus::Result<()> {
                let connection = bus.connect()?;
                let dbus = DBusProxy::new(&connection)?;
                let changes = dbus.receive_name_owner_changed_with_args(&[(0, TRAY_WATCHER)])?;
                // Whatever happened between the first look and now.
                on_change(Self::watcher_has_owner(&dbus));
                for change in changes {
                    if let Ok(args) = change.args() {
                        on_change(args.new_owner().is_some());
                    }
                }
                Ok(())
            };
            let spawned = std::thread::Builder::new().name("rt-tray-host".into()).spawn(move || {
                if let Err(err) = watch() {
                    eprintln!("not watching for a tray host: {err}");
                }
            });
            if let Err(err) = spawned {
                eprintln!("not watching for a tray host: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[derive(Default)]
    struct FakeEnv(HashMap<&'static str, &'static str>);

    impl FakeEnv {
        fn with(vars: &[(&'static str, &'static str)]) -> Self {
            FakeEnv(vars.iter().copied().collect())
        }
    }

    impl Env for FakeEnv {
        fn var(&self, name: &str) -> Option<String> {
            self.0
                .get(name)
                .filter(|value| !value.is_empty())
                .map(|v| v.to_string())
        }
    }

    #[derive(Default)]
    struct FakeBus {
        portal: Option<u32>,
        tray: bool,
        portal_asked: AtomicUsize,
        watcher: Mutex<Option<TrayHostListener>>,
    }

    impl FakeBus {
        fn with_portal(version: u32) -> Self {
            FakeBus {
                portal: Some(version),
                tray: true,
                ..FakeBus::default()
            }
        }

        /// The tray host appears (`true`) or goes away (`false`).
        fn tray_host_changes(&self, present: bool) {
            (self.watcher.lock().unwrap().as_ref().expect("a watcher"))(present);
        }
    }

    impl DesktopBus for FakeBus {
        fn global_shortcuts_version(&self) -> Option<u32> {
            self.portal_asked.fetch_add(1, Ordering::SeqCst);
            self.portal
        }

        fn tray_host_present(&self) -> bool {
            self.tray
        }

        fn watch_tray_host(&self, on_change: TrayHostListener) {
            *self.watcher.lock().unwrap() = Some(on_change);
        }
    }

    const WAYLAND: &[(&str, &str)] = &[("WAYLAND_DISPLAY", "wayland-0"), ("XDG_SESSION_TYPE", "wayland")];

    // Display server

    #[test]
    fn a_wayland_display_is_wayland() {
        assert_eq!(
            display_server(&FakeEnv::with(&[("WAYLAND_DISPLAY", "wayland-0")])),
            DisplayServer::Wayland
        );
    }

    #[test]
    fn a_wayland_session_type_is_wayland_even_without_the_display_variable() {
        assert_eq!(
            display_server(&FakeEnv::with(&[("XDG_SESSION_TYPE", "wayland")])),
            DisplayServer::Wayland
        );
    }

    #[test]
    fn a_display_alone_is_x11() {
        assert_eq!(display_server(&FakeEnv::with(&[("DISPLAY", ":0")])), DisplayServer::X11);
        assert_eq!(
            display_server(&FakeEnv::with(&[("DISPLAY", ":0"), ("XDG_SESSION_TYPE", "x11")])),
            DisplayServer::X11
        );
    }

    #[test]
    fn xwayland_is_still_wayland() {
        // Both are set under Wayland whenever an X11 app can run; the hotkeys of X11 wouldn't see the other windows.
        assert_eq!(
            display_server(&FakeEnv::with(&[("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "wayland-0")])),
            DisplayServer::Wayland
        );
        assert_eq!(
            display_server(&FakeEnv::with(&[("DISPLAY", ":0"), ("XDG_SESSION_TYPE", "wayland")])),
            DisplayServer::Wayland
        );
    }

    #[test]
    fn no_display_is_no_display() {
        assert_eq!(display_server(&FakeEnv::default()), DisplayServer::NoDisplay);
        // A login over ssh or on a console.
        assert_eq!(
            display_server(&FakeEnv::with(&[("XDG_SESSION_TYPE", "tty")])),
            DisplayServer::NoDisplay
        );
        assert_eq!(
            display_server(&FakeEnv::with(&[("XDG_SESSION_TYPE", "x11")])),
            DisplayServer::NoDisplay
        );
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        assert_eq!(
            display_server(&FakeEnv::with(&[("WAYLAND_DISPLAY", ""), ("DISPLAY", ":0")])),
            DisplayServer::X11
        );
        assert_eq!(
            display_server(&FakeEnv::with(&[("WAYLAND_DISPLAY", ""), ("DISPLAY", "")])),
            DisplayServer::NoDisplay
        );
    }

    // Session

    #[test]
    fn wayland_asks_the_bus_for_the_portal_and_keeps_its_version() {
        let bus = FakeBus::with_portal(2);
        let session = detect_for(display_server(&FakeEnv::with(WAYLAND)), &bus);
        assert_eq!(session.portal_version, Some(2));
        assert_eq!(bus.portal_asked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wayland_without_a_portal_has_no_version() {
        let session = detect_for(display_server(&FakeEnv::with(WAYLAND)), &FakeBus::default());
        assert_eq!(session.display, DisplayServer::Wayland);
        assert_eq!(session.portal_version, None);
    }

    #[test]
    fn x11_and_a_missing_display_never_ask_for_the_portal() {
        for vars in [&[("DISPLAY", ":0")][..], &[][..]] {
            let bus = FakeBus::with_portal(2);
            let session = detect_for(display_server(&FakeEnv::with(vars)), &bus);
            assert_eq!(session.portal_version, None, "{vars:?}");
            assert_eq!(bus.portal_asked.load(Ordering::SeqCst), 0, "{vars:?}");
        }
    }

    #[test]
    fn this_os_decides_whether_there_is_a_session_to_look_at() {
        // Wayland variables on a Mac or a Windows machine mean nothing: no portal is asked, the plugin is used.
        let bus = FakeBus::with_portal(2);
        let session = detect(&FakeEnv::with(WAYLAND), &bus);
        if cfg!(target_os = "linux") {
            assert_eq!(session.display, DisplayServer::Wayland);
        } else {
            assert_eq!(session.display, DisplayServer::Native);
            assert_eq!(session.portal_version, None);
            assert_eq!(bus.portal_asked.load(Ordering::SeqCst), 0);
            assert_eq!(choose_backend(&session), HotkeyBackendKind::Plugin);
        }
    }

    // Backend choice

    fn session(display: DisplayServer, portal_version: Option<u32>) -> Session {
        Session {
            display,
            portal_version,
        }
    }

    #[test]
    fn x11_uses_the_plugin_whether_or_not_a_portal_is_there() {
        assert_eq!(
            choose_backend(&session(DisplayServer::X11, None)),
            HotkeyBackendKind::Plugin
        );
        assert_eq!(
            choose_backend(&session(DisplayServer::X11, Some(2))),
            HotkeyBackendKind::Plugin
        );
    }

    #[test]
    fn windows_and_macos_use_the_plugin() {
        assert_eq!(
            choose_backend(&session(DisplayServer::Native, None)),
            HotkeyBackendKind::Plugin
        );
    }

    #[test]
    fn wayland_with_the_portal_uses_it_in_either_version() {
        for version in [1, 2] {
            assert_eq!(
                choose_backend(&session(DisplayServer::Wayland, Some(version))),
                HotkeyBackendKind::Portal,
                "v{version}"
            );
        }
    }

    #[test]
    fn wayland_without_the_portal_has_no_hotkeys() {
        assert_eq!(
            choose_backend(&session(DisplayServer::Wayland, None)),
            HotkeyBackendKind::None
        );
    }

    #[test]
    fn no_display_has_no_hotkeys() {
        assert_eq!(
            choose_backend(&session(DisplayServer::NoDisplay, None)),
            HotkeyBackendKind::None
        );
    }

    #[test]
    fn the_shortcut_plugin_is_only_wanted_where_it_works() {
        assert!(wants_shortcut_plugin(&FakeEnv::with(&[("DISPLAY", ":0")])) || !cfg!(target_os = "linux"));
        assert!(!wants_shortcut_plugin(&FakeEnv::with(WAYLAND)) || !cfg!(target_os = "linux"));
        assert!(!wants_shortcut_plugin(&FakeEnv::default()) || !cfg!(target_os = "linux"));
    }

    #[test]
    fn the_plugin_follows_the_backend_choice() {
        // Whatever starts the plugin and whatever picks the backend must agree.
        for vars in [&[("DISPLAY", ":0")][..], WAYLAND, &[][..]] {
            let env = FakeEnv::with(vars);
            let chosen = choose_backend(&detect(&env, &FakeBus::with_portal(2)));
            assert_eq!(
                wants_shortcut_plugin(&env),
                chosen == HotkeyBackendKind::Plugin,
                "{vars:?}"
            );
        }
    }

    // Tray host

    #[test]
    fn the_tray_host_is_read_at_the_start() {
        let with = FakeBus {
            tray: true,
            ..FakeBus::default()
        };
        assert!(start(&FakeEnv::default(), &with, || {}).tray_available());
        assert!(!start(&FakeEnv::default(), &FakeBus::default(), || {}).tray_available());
    }

    #[test]
    fn a_tray_host_that_shows_up_later_turns_the_tray_on_and_says_so() {
        let bus = FakeBus::default();
        let changes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&changes);
        let state = start(&FakeEnv::default(), &bus, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert!(!state.tray_available());

        bus.tray_host_changes(true);
        assert!(state.tray_available());
        assert_eq!(changes.load(Ordering::SeqCst), 1);

        bus.tray_host_changes(false);
        assert!(!state.tray_available());
        assert_eq!(changes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn hearing_the_same_thing_twice_is_not_news() {
        let bus = FakeBus {
            tray: true,
            ..FakeBus::default()
        };
        let changes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&changes);
        let state = start(&FakeEnv::default(), &bus, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        bus.tray_host_changes(true);
        bus.tray_host_changes(true);
        assert!(state.tray_available());
        assert_eq!(changes.load(Ordering::SeqCst), 0);
    }

    // The command that starts the app

    #[test]
    fn a_plain_install_runs_by_its_name() {
        assert_eq!(launch_command(&FakeEnv::default()), "repeat-that");
    }

    #[test]
    fn a_flatpak_runs_through_flatpak() {
        let env = FakeEnv::with(&[("FLATPAK_ID", "me.roboroads.RepeatThat")]);
        assert_eq!(launch_command(&env), "flatpak run me.roboroads.RepeatThat");
    }

    #[test]
    fn an_appimage_runs_as_its_own_file_in_quotes() {
        let env = FakeEnv::with(&[("APPIMAGE", "/home/me/My Apps/Repeat-That.AppImage")]);
        assert_eq!(launch_command(&env), r#""/home/me/My Apps/Repeat-That.AppImage""#);
    }

    #[test]
    fn quotes_and_shell_characters_in_the_appimage_path_are_escaped() {
        let env = FakeEnv::with(&[("APPIMAGE", r#"/tmp/a"b$c`d\e.AppImage"#)]);
        assert_eq!(launch_command(&env), r#""/tmp/a\"b\$c\`d\\e.AppImage""#);
    }

    #[test]
    fn the_flatpak_wins_over_a_stray_appimage_variable() {
        let env = FakeEnv::with(&[("FLATPAK_ID", "me.roboroads.RepeatThat"), ("APPIMAGE", "/x.AppImage")]);
        assert_eq!(launch_command(&env), "flatpak run me.roboroads.RepeatThat");
    }

    #[test]
    fn each_hotkey_action_has_its_own_command() {
        let commands = hotkey_commands(&FakeEnv::default());
        assert_eq!(commands.replay, "repeat-that --replay");
        assert_eq!(commands.stop, "repeat-that --stop");
        assert_eq!(commands.catch_up, "repeat-that --catch-up");
    }

    #[test]
    fn the_commands_follow_the_package() {
        let env = FakeEnv::with(&[("FLATPAK_ID", "me.roboroads.RepeatThat")]);
        assert_eq!(
            hotkey_commands(&env).catch_up,
            "flatpak run me.roboroads.RepeatThat --catch-up"
        );
    }

    /// The real bus code, against a bus of its own.
    #[cfg(target_os = "linux")]
    mod on_a_bus {
        use std::sync::mpsc;
        use std::time::Duration;

        use zbus::blocking::Connection;
        use zbus::blocking::connection::Builder;

        use super::super::DesktopBus;
        use super::super::linux::SessionBus;
        use crate::private_bus::PrivateBus;

        const SOON: Duration = Duration::from_secs(10);

        /// A portal that has the shortcuts interface in version `0`.
        struct Shortcuts(u32);

        #[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
        impl Shortcuts {
            #[zbus(property, name = "version")]
            fn version(&self) -> u32 {
                self.0
            }
        }

        /// A portal that has something else.
        struct Settings;

        #[zbus::interface(name = "org.freedesktop.portal.Settings")]
        impl Settings {
            #[zbus(property, name = "version")]
            fn version(&self) -> u32 {
                2
            }
        }

        fn portal_with(bus: &PrivateBus, interface: impl zbus::object_server::Interface) -> Connection {
            Builder::address(bus.address.as_str())
                .unwrap()
                .name("org.freedesktop.portal.Desktop")
                .unwrap()
                .serve_at("/org/freedesktop/portal/desktop", interface)
                .unwrap()
                .build()
                .unwrap()
        }

        fn tray_host_on(bus: &PrivateBus) -> Connection {
            Builder::address(bus.address.as_str())
                .unwrap()
                .name("org.kde.StatusNotifierWatcher")
                .unwrap()
                .build()
                .unwrap()
        }

        #[test]
        fn an_empty_bus_has_no_tray_host_and_no_portal() {
            let Some(bus) = PrivateBus::start() else { return };
            let session = SessionBus::at(&bus.address);
            assert!(!session.tray_host_present());
            assert_eq!(session.global_shortcuts_version(), None);
        }

        #[test]
        fn a_status_notifier_watcher_is_a_tray_host() {
            let Some(bus) = PrivateBus::start() else { return };
            let _host = tray_host_on(&bus);
            assert!(SessionBus::at(&bus.address).tray_host_present());
        }

        #[test]
        fn a_tray_host_coming_and_going_is_heard() {
            let Some(bus) = PrivateBus::start() else { return };
            let session = SessionBus::at(&bus.address);
            let (heard, changes) = mpsc::channel();
            session.watch_tray_host(Box::new(move |present| heard.send(present).unwrap()));
            // What is true when the watch starts.
            assert_eq!(changes.recv_timeout(SOON), Ok(false));

            let host = tray_host_on(&bus);
            assert_eq!(changes.recv_timeout(SOON), Ok(true));
            drop(host);
            assert_eq!(changes.recv_timeout(SOON), Ok(false));
        }

        #[test]
        fn a_tray_host_that_was_there_before_the_watch_is_heard_too() {
            let Some(bus) = PrivateBus::start() else { return };
            let _host = tray_host_on(&bus);
            let (heard, changes) = mpsc::channel();
            SessionBus::at(&bus.address).watch_tray_host(Box::new(move |present| heard.send(present).unwrap()));
            assert_eq!(changes.recv_timeout(SOON), Ok(true));
        }

        #[test]
        fn a_portal_without_shortcuts_is_not_a_shortcuts_portal() {
            let Some(bus) = PrivateBus::start() else { return };
            let _portal = portal_with(&bus, Settings);
            assert_eq!(SessionBus::at(&bus.address).global_shortcuts_version(), None);
        }

        #[test]
        fn a_shortcuts_portal_reports_its_version() {
            for version in [1, 2] {
                let Some(bus) = PrivateBus::start() else { return };
                let _portal = portal_with(&bus, Shortcuts(version));
                assert_eq!(
                    SessionBus::at(&bus.address).global_shortcuts_version(),
                    Some(version),
                    "v{version}"
                );
            }
        }
    }
}
