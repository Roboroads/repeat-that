//! Global hotkeys through the XDG GlobalShortcuts portal, which is how a Wayland desktop lets an app have them: the app
//! doesn't grab keys, it tells the desktop which actions it has, and the desktop asks the user which keys to give them
//! (KDE Plasma, GNOME 48 and later). When the user presses one, the desktop sends `Activated`, and `Deactivated` when it
//! is let go, which are the same two edges the plugin reports on X11, so the press tracker and everything behind it
//! work as before.
//!
//! The keys the app asks for are only a preference, taken from the stored hotkeys. What the desktop gives, and what the
//! user changes later in the system settings, is the desktop's: the window shows it (`Status.hotkeyTriggers`) and has a
//! button that opens the desktop's own settings page.
//!
//! [`PortalBackend`] is the [`ShortcutBackend`] the hotkey manager drives. A thread of the async runtime talks to the
//! portal (see `worker`) and feeds what happens back through a [`PortalLink`]; the two sides share one small state
//! machine, which is what the tests below drive.
//!
//! A new session doesn't always bind. KDE's portal (Plasma 6.1 and later) starts each session with the shortcuts it
//! kept for the app, already working, and Plasma 6.3 opens System Settings on its Shortcuts page for every bind, so
//! binding at every start opened it at every start. There a session lists the shortcuts first and binds only when one
//! is missing (see [`sessions_keep_shortcuts`]).

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::async_runtime::{Receiver, Sender};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

use crate::actions::AppAction;
use crate::hotkeys::{EdgeHandler, KeyEdge, ShortcutBackend};
use crate::ipc::{ConfigureOutcome, HOTKEY_COUNT, HotkeyBackendKind, HotkeyTriggers};
use crate::locking::lock;
use crate::session::{Env, ProcessEnv};
use crate::settings::{HOTKEY_ACTIONS, HOTKEYS, parse_accel};

/// The ID the app tells the portal it has: the name of its desktop file without `.desktop`.
///
/// This depends on the Linux packages (step P of the Linux plan, §3.4): they must install the desktop file as
/// `me.roboroads.repeat-that.desktop`, where the bundler would name it after the binary. The portal shows the app's name
/// and icon next to its shortcuts from that file, and a name that doesn't match one gets them shown without.
pub const HOST_APP_ID: &str = "me.roboroads.repeat-that";

/// The component Plasma's portal keeps the app's shortcuts in, which is the app ID it knows the app by: the one the app
/// registered ([`HOST_APP_ID`]), or in a Flatpak the Flatpak ID, which it reads from the sandbox (an app inside one
/// can't register another). `None` in a Flatpak that has no ID in its environment, where there is no component to name.
fn kde_component(env: &dyn Env, in_flatpak: bool) -> Option<String> {
    if in_flatpak {
        env.var("FLATPAK_ID")
    } else {
        Some(HOST_APP_ID.to_string())
    }
}

/// Whether what the portal lists for a new session is what works in it, so that a session which finds all of its
/// shortcuts listed has nothing to bind. KDE's portal lists the actions the session itself has registered with
/// kglobalaccel. From Plasma 6.1 on it registers the ones it kept for the app when it makes the session; Plasma 5.27
/// and 6.0 register nothing before a bind and list nothing, so a session there binds as before. The specification only
/// says that the list holds what an earlier session bound, not that it works in this one, and GNOME's portal grabs the
/// keys in the bind (and lists nothing before it), so everywhere else a session binds without asking. The portal picks
/// KDE's backend by `XDG_CURRENT_DESKTOP`, and so does this.
fn sessions_keep_shortcuts(env: &dyn Env) -> bool {
    env.var("XDG_CURRENT_DESKTOP")
        .is_some_and(|desktops| desktops.split(':').any(|name| name.trim().eq_ignore_ascii_case("kde")))
}

/// Whether `listed` has a shortcut for each of `requests`, whatever its key. The key is the desktop's to give and the
/// user's to change, and the portal describes it in its own words, which can't be compared with a preferred trigger in
/// the specification's format. Binding again wouldn't change it either: the portal keeps the key a shortcut has.
fn has_every_action(requests: &[ShortcutRequest], listed: &[Assigned]) -> bool {
    requests
        .iter()
        .all(|request| listed.iter().any(|shortcut| shortcut.id == request.id))
}

/// How long a new session waits for the portal to list the shortcuts it started with. KDE's portal answers at once; one
/// that doesn't answer in time gets the bind instead, so the hotkeys never wait on it for long.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait for the portal to answer a request to open its settings.
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait for kglobalaccel when giving the shortcuts their name in KDE.
const KDE_NAME_TIMEOUT: Duration = Duration::from_secs(10);

/// How long after the portal is not found, or its session ended, to look again: quickly at first, so a portal that is
/// slow at login still gets its hotkeys, then once a minute. The last delay repeats.
const RETRY: &[Duration] = &[
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

/// Room for the requests waiting for the portal thread. A few are ever queued.
const QUEUE: usize = 8;

/// `shortcut` in the format of the shortcuts specification (`CTRL+ALT+space`): modifiers in capitals, then the key by
/// its X keysym name. `None` for a key with no name there; the portal is then asked for the action without a
/// preferred key, and the user picks one.
pub fn xdg_trigger(shortcut: &Shortcut) -> Option<String> {
    let mut parts = Vec::new();
    if shortcut.mods.contains(Modifiers::CONTROL) {
        parts.push("CTRL".to_string());
    }
    if shortcut.mods.contains(Modifiers::ALT) {
        parts.push("ALT".to_string());
    }
    if shortcut.mods.contains(Modifiers::SHIFT) {
        parts.push("SHIFT".to_string());
    }
    if shortcut.mods.intersects(Modifiers::SUPER | Modifiers::META) {
        parts.push("LOGO".to_string());
    }
    parts.push(keysym(shortcut.key)?);
    Some(parts.join("+"))
}

/// The X keysym name of `code`, a physical key as the recorder in the window reports it.
fn keysym(code: Code) -> Option<String> {
    let name = code.to_string();
    if let Some(letter) = name.strip_prefix("Key")
        && letter.len() == 1
    {
        return Some(letter.to_ascii_lowercase());
    }
    if let Some(digit) = name.strip_prefix("Digit") {
        return Some(digit.to_string());
    }
    if let Some(number) = name.strip_prefix('F')
        && number.parse::<u8>().is_ok()
    {
        return Some(name);
    }
    if let Some(key) = name.strip_prefix("Numpad") {
        return match key {
            "Add" => Some("KP_Add".into()),
            "Subtract" => Some("KP_Subtract".into()),
            "Multiply" => Some("KP_Multiply".into()),
            "Divide" => Some("KP_Divide".into()),
            "Decimal" => Some("KP_Decimal".into()),
            "Enter" => Some("KP_Enter".into()),
            digit if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() => Some(format!("KP_{digit}")),
            _ => None,
        };
    }
    let keysym = match name.as_str() {
        "Space" => "space",
        "Enter" => "Return",
        "Backspace" => "BackSpace",
        "Escape" => "Escape",
        "Tab" => "Tab",
        "Delete" => "Delete",
        "Insert" => "Insert",
        "Home" => "Home",
        "End" => "End",
        "PageUp" => "Prior",
        "PageDown" => "Next",
        "ArrowUp" => "Up",
        "ArrowDown" => "Down",
        "ArrowLeft" => "Left",
        "ArrowRight" => "Right",
        "Backquote" => "grave",
        "Minus" => "minus",
        "Equal" => "equal",
        "Comma" => "comma",
        "Period" => "period",
        "Slash" => "slash",
        "Backslash" => "backslash",
        "Semicolon" => "semicolon",
        "Quote" => "apostrophe",
        "BracketLeft" => "bracketleft",
        "BracketRight" => "bracketright",
        _ => return None,
    };
    Some(keysym.to_string())
}

/// One action to ask the portal for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutRequest {
    /// The stable ID the portal knows the action by, and the command-line flag of it.
    pub id: &'static str,
    /// What the portal shows the user next to the key.
    pub description: &'static str,
    pub preferred_trigger: Option<String>,
}

/// A shortcut as the portal says it is bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assigned {
    pub id: String,
    /// How the user triggers it, as the desktop describes it; empty when it has no key.
    pub trigger: String,
}

/// What the backend asks of the thread that talks to the portal.
#[derive(Debug)]
pub enum Request {
    /// Binds these shortcuts, which may show the user a dialog.
    Bind(Vec<ShortcutRequest>),
    /// Has the desktop show where the keys are changed. The answer goes back through the channel, unless `deadline` has
    /// passed: whoever asked has given up by then, and opening a page nobody asked for would only surprise the user.
    Configure {
        reply: mpsc::Sender<Result<ConfigureOutcome, String>>,
        deadline: Instant,
    },
}

/// Where the portal stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Availability {
    /// Looking for the portal for the first time. The hotkeys are expected, so they read as the portal's, without keys
    /// yet, instead of flashing the warning that there are none.
    #[default]
    Probing,
    Available {
        /// The version of the GlobalShortcuts interface.
        version: u32,
    },
    /// There is no usable portal right now: it isn't there (yet), it has no shortcuts backend, or its session ended. It
    /// is looked for again, so a portal that is slow at login, or comes later, still gets its hotkeys.
    Unavailable(String),
}

#[derive(Default)]
struct State {
    handlers: [Option<Arc<EdgeHandler>>; HOTKEY_COUNT],
    /// The key each action is asked for, in the portal's format; none when the stored one is empty or unreadable.
    preferred: [Option<String>; HOTKEY_COUNT],
    /// The key the portal says each action has.
    assigned: [Option<String>; HOTKEY_COUNT],
    /// What the last `Bind` asked for, so asking for the same again doesn't.
    sent: Option<Vec<ShortcutRequest>>,
    availability: Availability,
    /// How many binds are queued or waiting for the user. While there is one, a dialog may be open.
    binds_pending: usize,
    /// The portal has answered a bind (even with no keys), so its shortcuts exist and its settings page has them.
    bound_once: bool,
}

impl State {
    /// Every action, each with the key it is asked for. An action without a stored key is still bound, so the user
    /// can give it one in the system settings.
    fn requests(&self) -> Vec<ShortcutRequest> {
        HOTKEYS
            .iter()
            .zip(&self.preferred)
            .map(|(def, preferred)| ShortcutRequest {
                id: def.command,
                description: def.label,
                preferred_trigger: preferred.clone(),
            })
            .collect()
    }
}

struct Shared {
    state: Mutex<State>,
    /// Called when what the window shows may have changed. Never called with the state locked.
    on_change: Box<dyn Fn() + Send + Sync>,
}

fn slot_of_action(action: AppAction) -> Option<usize> {
    HOTKEY_ACTIONS.iter().position(|&a| a == action)
}

fn slot_of_id(id: &str) -> Option<usize> {
    HOTKEYS.iter().position(|def| def.command == id)
}

/// The portal side of the hotkeys: what the manager registers goes to the portal thread, and what the portal does
/// comes back through the [`PortalLink`].
pub struct PortalBackend {
    shared: Arc<Shared>,
    requests: Sender<Request>,
    /// Starts the thread that talks to the portal. The first [`commit`](ShortcutBackend::commit) runs it, not the
    /// backend's creation: by then the stored keys are registered as preferences, so the very first bind carries them
    /// instead of asking the user twice.
    start: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// What the portal thread tells the backend.
#[derive(Clone)]
pub struct PortalLink(Arc<Shared>);

/// A backend that has no thread behind it yet, with the link to feed it and the requests it makes. `on_change` is called
/// whenever what the window shows about the keys may have changed.
pub fn channel(on_change: impl Fn() + Send + Sync + 'static) -> (PortalBackend, PortalLink, Receiver<Request>) {
    let shared = Arc::new(Shared {
        state: Mutex::default(),
        on_change: Box::new(on_change),
    });
    let (requests, receiver) = tauri::async_runtime::channel(QUEUE);
    let backend = PortalBackend {
        shared: Arc::clone(&shared),
        requests,
        start: Mutex::new(None),
    };
    (backend, PortalLink(shared), receiver)
}

/// Looks for the portal, and talks to it once it is there, from the first `commit` on. Returns at once: a portal that is
/// slow to answer at login is waited for on a thread of the async runtime, not by the app's start.
pub fn start(on_change: impl Fn() + Send + Sync + 'static) -> PortalBackend {
    start_on(
        on_change,
        zbus::Connection::session(),
        RETRY,
        sessions_keep_shortcuts(&ProcessEnv).then_some(LIST_TIMEOUT),
    )
}

/// [`start`] on the bus `connect` reaches, looking again after each of `retry` (the last one repeats). `list_for` is
/// how long a new session waits for the portal to list the shortcuts it started with, on a portal whose list says what
/// works in the session (see [`sessions_keep_shortcuts`]); `None` elsewhere, where a session binds without asking.
fn start_on(
    on_change: impl Fn() + Send + Sync + 'static,
    connect: impl Future<Output = zbus::Result<zbus::Connection>> + Send + 'static,
    retry: &'static [Duration],
    list_for: Option<Duration>,
) -> PortalBackend {
    let (backend, link, requests) = channel(on_change);
    *lock(&backend.start) = Some(Box::new(move || {
        tauri::async_runtime::spawn(async move {
            match connect.await {
                Ok(connection) => worker::run(&link, requests, connection, retry, list_for).await,
                Err(err) => link.unavailable(format!("could not reach the session bus: {err}")),
            }
        });
    }));
    backend
}

impl PortalBackend {
    /// Queues a bind that the state already counts as pending. One that can't be queued isn't pending after all, and
    /// can be asked for again.
    fn queue_bind(&self, requests: Vec<ShortcutRequest>) -> Result<(), String> {
        self.requests.try_send(Request::Bind(requests)).map_err(|err| {
            let mut state = lock(&self.shared.state);
            state.binds_pending = state.binds_pending.saturating_sub(1);
            state.sent = None;
            format!("could not ask the portal: {err}")
        })
    }
}

impl ShortcutBackend for PortalBackend {
    fn attach(&self, handlers: Vec<(AppAction, EdgeHandler)>) {
        let mut state = lock(&self.shared.state);
        for (action, handler) in handlers {
            if let Some(slot) = slot_of_action(action) {
                state.handlers[slot] = Some(Arc::new(handler));
            }
        }
    }

    /// Notes the key to ask for. Every action is bound whatever it is stored as, see [`State::requests`], so this only
    /// sets a preference.
    fn register(&self, action: AppAction, accel: &str, _on_edge: EdgeHandler) -> Result<(), String> {
        let slot = slot_of_action(action).ok_or_else(|| format!("{action:?} has no hotkey"))?;
        let preferred = parse_accel(accel).ok().and_then(|shortcut| xdg_trigger(&shortcut));
        lock(&self.shared.state).preferred[slot] = preferred;
        Ok(())
    }

    fn unregister(&self, action: AppAction, _accel: &str) {
        if let Some(slot) = slot_of_action(action) {
            lock(&self.shared.state).preferred[slot] = None;
        }
    }

    fn commit(&self) {
        let start = lock(&self.start).take();
        if let Some(start) = start {
            start();
        }
        let requests = {
            let mut state = lock(&self.shared.state);
            // Until the portal is there, the session that gets made binds what the state holds then.
            if !matches!(state.availability, Availability::Available { .. }) {
                return;
            }
            let requests = state.requests();
            if state.sent.as_ref() == Some(&requests) {
                return;
            }
            state.sent = Some(requests.clone());
            state.binds_pending += 1;
            requests
        };
        if let Err(err) = self.queue_bind(requests) {
            eprintln!("{err}");
        }
    }

    fn kind(&self) -> HotkeyBackendKind {
        if matches!(lock(&self.shared.state).availability, Availability::Unavailable(_)) {
            HotkeyBackendKind::None
        } else {
            HotkeyBackendKind::Portal
        }
    }

    fn triggers(&self) -> HotkeyTriggers {
        let state = lock(&self.shared.state);
        let mut triggers = HotkeyTriggers::default();
        for (slot, trigger) in triggers.slots_mut().into_iter().zip(&state.assigned) {
            slot.clone_from(trigger);
        }
        triggers
    }

    /// The window has no recorder under the portal, so there is nothing to switch the hotkeys off for.
    fn can_suspend(&self) -> bool {
        false
    }

    /// Gets the user to the keys. While the shortcuts dialog is open it does nothing but say so: asking would queue behind
    /// the dialog and open a page long after it was asked for. Until the portal has taken the shortcuts (the dialog was
    /// closed, or it failed) the way to the keys is that dialog again; after that it is the desktop's settings page,
    /// which only version 2 of the portal can open.
    fn configure(&self) -> Result<ConfigureOutcome, String> {
        enum Plan {
            Dialog(Vec<ShortcutRequest>),
            SettingsPage,
        }
        let plan = {
            let mut state = lock(&self.shared.state);
            let version = match state.availability {
                Availability::Available { version } => version,
                // Not known yet: asking again in a moment will be.
                Availability::Probing => return Ok(ConfigureOutcome::Busy),
                Availability::Unavailable(_) => return Ok(ConfigureOutcome::Unsupported),
            };
            if state.binds_pending > 0 {
                return Ok(ConfigureOutcome::Busy);
            }
            if !state.bound_once {
                let requests = state.requests();
                state.sent = Some(requests.clone());
                state.binds_pending += 1;
                Plan::Dialog(requests)
            } else if version < 2 {
                // `ConfigureShortcuts` came with version 2 of the interface.
                return Ok(ConfigureOutcome::Unsupported);
            } else {
                Plan::SettingsPage
            }
        };
        match plan {
            Plan::Dialog(requests) => {
                self.queue_bind(requests)?;
                Ok(ConfigureOutcome::Opened)
            }
            Plan::SettingsPage => {
                let (reply, answer) = mpsc::channel();
                let deadline = Instant::now() + CONFIGURE_TIMEOUT;
                self.requests
                    .try_send(Request::Configure { reply, deadline })
                    .map_err(|err| format!("could not ask the portal: {err}"))?;
                answer
                    .recv_timeout(CONFIGURE_TIMEOUT)
                    .map_err(|_| "the desktop did not answer".to_string())?
            }
        }
    }
}

impl Shared {
    /// Runs the handler of `id`'s action with `edge`. A shortcut nobody asked for, and every shortcut while the portal
    /// isn't there, does nothing.
    fn edge(&self, id: &str, edge: KeyEdge) {
        let Some(slot) = slot_of_id(id) else { return };
        let handler = {
            let state = lock(&self.state);
            if matches!(state.availability, Availability::Unavailable(_)) {
                return;
            }
            state.handlers[slot].clone()
        };
        // Outside the lock: the action may do anything, including registering hotkeys.
        if let Some(handler) = handler {
            handler(edge);
        }
    }

    /// Records what the portal says is bound to `shortcuts`; with `replace` the actions it doesn't list have no key.
    fn assign(&self, shortcuts: &[Assigned], replace: bool) {
        {
            let mut state = lock(&self.state);
            if replace {
                state.assigned = Default::default();
            }
            for shortcut in shortcuts {
                if let Some(slot) = slot_of_id(&shortcut.id) {
                    state.assigned[slot] = Some(shortcut.trigger.clone()).filter(|trigger| !trigger.is_empty());
                }
            }
        }
        (self.on_change)();
    }
}

impl PortalLink {
    /// A shortcut became active: its key went down.
    pub fn activated(&self, id: &str) {
        self.0.edge(id, KeyEdge::Pressed);
    }

    /// A shortcut is not active anymore: its key was let go.
    pub fn deactivated(&self, id: &str) {
        self.0.edge(id, KeyEdge::Released);
    }

    /// A session with the portal (of the interface in `version`) is up, with the shortcuts in `listed` when the portal
    /// was asked which it started with. Whatever was asked while there was none is out of date, so the state says what
    /// to bind now: the first bind of the session, which is counted as pending. `None` when the session has every
    /// action already: their keys are the answer, and a bind would change nothing but could show the user a dialog or
    /// the desktop's settings.
    pub fn session_up(&self, version: u32, listed: Option<&[Assigned]>) -> Option<Vec<ShortcutRequest>> {
        let (requests, kept) = {
            let mut state = lock(&self.0.state);
            state.availability = Availability::Available { version };
            let requests = state.requests();
            state.sent = Some(requests.clone());
            let kept = listed.filter(|listed| has_every_action(&requests, listed));
            if kept.is_some() {
                state.binds_pending = 0;
                state.bound_once = true;
            } else {
                state.binds_pending = 1;
            }
            (requests, kept)
        };
        match kept {
            Some(listed) => {
                self.0.assign(listed, true);
                None
            }
            None => {
                (self.0.on_change)();
                Some(requests)
            }
        }
    }

    /// The portal answered a bind with the shortcuts that are bound now.
    pub fn bound(&self, shortcuts: &[Assigned]) {
        {
            let mut state = lock(&self.0.state);
            state.binds_pending = state.binds_pending.saturating_sub(1);
            state.bound_once = true;
        }
        self.0.assign(shortcuts, true);
    }

    /// The user closed the dialog without choosing. The actions keep the keys they had, and asking again is allowed.
    pub fn bind_dismissed(&self) {
        {
            let mut state = lock(&self.0.state);
            state.binds_pending = state.binds_pending.saturating_sub(1);
            state.sent = None;
        }
        (self.0.on_change)();
    }

    /// A bind failed, but the session is still there. The keys it had are kept, and asking again is allowed. The session
    /// ending is a different thing, see [`unavailable`](Self::unavailable).
    pub fn bind_failed(&self, reason: &str) {
        eprintln!("{reason}");
        self.bind_dismissed();
    }

    /// The portal says some shortcuts changed, usually because the user changed them in the system settings.
    pub fn changed(&self, shortcuts: &[Assigned]) {
        self.0.assign(shortcuts, false);
    }

    /// There is no usable portal now: the hotkeys are gone, and the window shows the commands instead, until a session is
    /// made again. Saying so again with the same reason is no news.
    pub fn unavailable(&self, reason: String) {
        {
            let mut state = lock(&self.0.state);
            if state.availability == Availability::Unavailable(reason.clone()) {
                return;
            }
            eprintln!("hotkeys through the portal are off: {reason}");
            state.availability = Availability::Unavailable(reason);
            state.assigned = Default::default();
            state.sent = None;
            state.binds_pending = 0;
            state.bound_once = false;
        }
        (self.0.on_change)();
    }
}

mod worker {
    //! The thread that talks to the portal: looks for it, makes the session, binds what the backend asks for and tells
    //! the link what happens. It looks again when the portal isn't there or the session ends. Its protocol is tested
    //! against a stand-in portal; `PortalLink` is the part with the rules.

    use ashpd::desktop::global_shortcuts::{
        BindShortcutsOptions, ConfigureShortcutsOptions, GlobalShortcuts, ListShortcutsOptions, NewShortcut, Shortcut,
    };
    use ashpd::desktop::{ResponseError, Session};
    use ashpd::{AppID, Error};
    use futures_util::future::{Either, select};
    use futures_util::stream::{self, Stream, StreamExt};
    use zbus::fdo::PropertiesProxy;
    use zbus::names::InterfaceName;

    use super::*;
    use crate::kglobalaccel;

    /// Where the portals live, and the interface the hotkeys use.
    const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
    const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
    const SHORTCUTS_INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

    enum Signal {
        Activated(String),
        Deactivated(String),
        Changed(Vec<Assigned>),
        Closed,
    }

    fn assigned(shortcuts: &[Shortcut]) -> Vec<Assigned> {
        shortcuts
            .iter()
            .map(|shortcut| Assigned {
                id: shortcut.id().to_string(),
                trigger: shortcut.trigger_description().to_string(),
            })
            .collect()
    }

    type Signals<'a> = std::pin::Pin<Box<dyn Stream<Item = Signal> + Send + 'a>>;

    /// The `version` the portal's GlobalShortcuts interface reports. Asked of the interface itself: a portal that is
    /// there but has no backend for shortcuts (GNOME before 48) answers for its other interfaces and fails for this one,
    /// and a library's idea of "the portal is there" would wrongly take it for one that has shortcuts.
    async fn probe_version(connection: &zbus::Connection) -> Result<u32, String> {
        let properties = PropertiesProxy::builder(connection)
            .destination(PORTAL_NAME)
            .and_then(|builder| builder.path(PORTAL_PATH))
            .map_err(|err| err.to_string())?
            .build()
            .await
            .map_err(|err| err.to_string())?;
        let interface = InterfaceName::try_from(SHORTCUTS_INTERFACE).map_err(|err| err.to_string())?;
        let version = properties
            .get(interface, "version")
            .await
            .map_err(|err| err.to_string())?;
        u32::try_from(version).map_err(|err| err.to_string())
    }

    /// Waits without holding up the async runtime's threads.
    async fn pause(delay: Duration) {
        let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(delay)).await;
    }

    /// Looks for the portal and keeps a session with it, for as long as the backend exists. When the portal isn't there
    /// or the session ends, the link is told, and it is looked for again after the next of `retry`; a session that was up
    /// starts the delays over. With `list_for` a new session first asks the portal which shortcuts it started with, and
    /// waits that long for the answer (see `start_on`).
    pub async fn run(
        link: &PortalLink,
        mut requests: Receiver<Request>,
        connection: zbus::Connection,
        retry: &'static [Duration],
        list_for: Option<Duration>,
    ) {
        let mut failures = 0_usize;
        loop {
            let mut established = false;
            match session(link, &mut requests, &connection, &mut established, list_for).await {
                // The backend is gone with the app.
                Ok(()) => return,
                Err(reason) => link.unavailable(reason),
            }
            failures = if established { 0 } else { failures + 1 };
            pause(retry[failures.saturating_sub(1).min(retry.len() - 1)]).await;
            if requests.is_closed() {
                return;
            }
        }
    }

    /// One session with the portal. Ends with `Ok` when the backend is gone, and with why otherwise. `established` says
    /// whether the session got as far as being up.
    async fn session(
        link: &PortalLink,
        requests: &mut Receiver<Request>,
        connection: &zbus::Connection,
        established: &mut bool,
        list_for: Option<Duration>,
    ) -> Result<(), String> {
        let version = probe_version(connection)
            .await
            .map_err(|err| format!("the shortcuts portal is not there: {err}"))?;
        // Portals know apps by their ID. A packaged app is told apart by its desktop file, an unpackaged one has to say
        // who it is. Older portals don't have the call; they find the app by other means. A Flatpak is known by its
        // Flatpak ID, which the portal reads from the sandbox, and the call is for apps outside one.
        if !rt_capture::in_flatpak() {
            match AppID::try_from(HOST_APP_ID) {
                Ok(id) => {
                    if let Err(err) = ashpd::register_host_app_with_connection(connection.clone(), id).await {
                        eprintln!("could not tell the portal the app's ID (fine on older portals): {err}");
                    }
                }
                Err(err) => eprintln!("{HOST_APP_ID} is not a valid app ID: {err}"),
            }
        }

        let portal = GlobalShortcuts::with_connection(connection.clone())
            .await
            .map_err(|err| format!("the shortcuts portal is not there: {err}"))?;
        let session = portal
            .create_session(Default::default())
            .await
            .map_err(|err| format!("could not start a shortcuts session: {err}"))?;
        let activated = portal
            .receive_activated()
            .await
            .map_err(|err| format!("could not listen for shortcuts: {err}"))?;
        let deactivated = portal
            .receive_deactivated()
            .await
            .map_err(|err| format!("could not listen for shortcuts: {err}"))?;
        let changed = portal
            .receive_shortcuts_changed()
            .await
            .map_err(|err| format!("could not listen for shortcut changes: {err}"))?;
        let closed = session
            .receive_closed()
            .await
            .map_err(|err| format!("could not listen for the session ending: {err}"))?;
        let mut signals: Signals<'_> = Box::pin(stream::select_all(vec![
            activated
                .map(|signal| Signal::Activated(signal.shortcut_id().to_string()))
                .boxed(),
            deactivated
                .map(|signal| Signal::Deactivated(signal.shortcut_id().to_string()))
                .boxed(),
            changed
                .map(|signal| Signal::Changed(assigned(signal.shortcuts())))
                .boxed(),
            closed.map(|_| Signal::Closed).boxed(),
        ]));

        // Asked while there was no session, and out of date now: what to bind is in the state.
        while requests.try_recv().is_ok() {}
        *established = true;
        let listed = match list_for {
            Some(limit) => list(&portal, &session, limit).await,
            None => None,
        };
        if let Some(shortcuts) = link.session_up(version, listed.as_deref()) {
            bind(&portal, &session, link, &shortcuts).await;
        }
        // Plasma 6.3's portal lists the shortcuts under the app ID once a session starts, see `kglobalaccel`, in a Flatpak
        // too: it names them after the Flatpak ID. Done after the first answer (the list, or the bind), when the portal
        // has registered them, whether the dialog was accepted or not. Later binds in the session leave the name alone.
        name_for_kde(connection);

        loop {
            let request = std::pin::pin!(requests.recv());
            let signal = std::pin::pin!(signals.next());
            match select(request, signal).await {
                Either::Left((None, _)) => return Ok(()),
                // Waits for the user if the desktop asks them.
                Either::Left((Some(Request::Bind(shortcuts)), _)) => bind(&portal, &session, link, &shortcuts).await,
                Either::Left((Some(Request::Configure { reply, deadline }), _)) => {
                    // The asker gave up waiting, and the page would open unasked.
                    if Instant::now() >= deadline {
                        continue;
                    }
                    let answer = match portal
                        .configure_shortcuts(&session, None, ConfigureShortcutsOptions::default())
                        .await
                    {
                        Ok(()) => Ok(ConfigureOutcome::Opened),
                        Err(Error::RequiresVersion(..)) => Ok(ConfigureOutcome::Unsupported),
                        Err(err) => Err(err.to_string()),
                    };
                    let _ = reply.send(answer);
                }
                Either::Right((Some(Signal::Activated(id)), _)) => link.activated(&id),
                Either::Right((Some(Signal::Deactivated(id)), _)) => link.deactivated(&id),
                Either::Right((Some(Signal::Changed(shortcuts)), _)) => link.changed(&shortcuts),
                Either::Right((Some(Signal::Closed) | None, _)) => {
                    return Err("the desktop ended the shortcuts session".to_string());
                }
            }
        }
    }

    /// Gives the shortcuts their name in KDE (see `kglobalaccel`) on a task of its own, with a time limit, so a desktop
    /// that is slow to answer never holds up the hotkeys or the settings button.
    fn name_for_kde(connection: &zbus::Connection) {
        let Some(component) = kde_component(&ProcessEnv, rt_capture::in_flatpak()) else {
            return;
        };
        let connection = connection.clone();
        tauri::async_runtime::spawn(async move {
            let name = std::pin::pin!(kglobalaccel::name_component(
                &connection,
                &component,
                kglobalaccel::APP_NAME
            ));
            match select(name, std::pin::pin!(pause(KDE_NAME_TIMEOUT))).await {
                Either::Left((Ok(_), _)) => {}
                Either::Left((Err(err), _)) => eprintln!("could not give the shortcuts their name in KDE: {err}"),
                Either::Right(_) => {
                    eprintln!("could not give the shortcuts their name in KDE: kglobalaccel did not answer")
                }
            }
        });
    }

    /// The shortcuts the session started with, or `None` when the portal can't say within `limit`. The session then
    /// binds, as it would without the list.
    async fn list(
        portal: &GlobalShortcuts,
        session: &Session<GlobalShortcuts>,
        limit: Duration,
    ) -> Option<Vec<Assigned>> {
        let answer = std::pin::pin!(async {
            portal
                .list_shortcuts(session, ListShortcutsOptions::default())
                .await
                .and_then(|request| request.response())
        });
        match select(answer, std::pin::pin!(pause(limit))).await {
            Either::Left((Ok(listed), _)) => Some(assigned(listed.shortcuts())),
            Either::Left((Err(err), _)) => {
                eprintln!("could not list the shortcuts, so they are bound: {err}");
                None
            }
            Either::Right(_) => {
                eprintln!("the portal did not list the shortcuts in time, so they are bound");
                None
            }
        }
    }

    /// Binds `shortcuts` and tells the link how it went. Only the session ending turns the hotkeys off: a bind that fails
    /// leaves the session, and the keys it already has, as they were.
    async fn bind(
        portal: &GlobalShortcuts,
        session: &Session<GlobalShortcuts>,
        link: &PortalLink,
        shortcuts: &[ShortcutRequest],
    ) {
        let new: Vec<NewShortcut> = shortcuts
            .iter()
            .map(|shortcut| {
                NewShortcut::new(shortcut.id, shortcut.description)
                    .preferred_trigger(shortcut.preferred_trigger.as_deref())
            })
            .collect();
        let answer = portal
            .bind_shortcuts(session, &new, None, BindShortcutsOptions::default())
            .await
            .and_then(|request| request.response());
        match answer {
            Ok(bound) => link.bound(&assigned(bound.shortcuts())),
            Err(Error::Response(ResponseError::Cancelled)) => link.bind_dismissed(),
            Err(err) => link.bind_failed(&format!("could not bind the shortcuts: {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::hotkeys::HotkeyManager;
    use crate::ipc::{HotkeyBindings, HotkeyState, HotkeyStates};
    use crate::session::fake::FakeEnv;

    fn trigger(accel: &str) -> Option<String> {
        xdg_trigger(&parse_accel(accel).unwrap())
    }

    // The component KDE keeps the shortcuts in

    #[test]
    fn a_flatpak_is_known_to_plasma_by_its_flatpak_id() {
        let env = FakeEnv::with(&[("FLATPAK_ID", "me.roboroads.RepeatThat")]);
        assert_eq!(kde_component(&env, true).as_deref(), Some("me.roboroads.RepeatThat"));
    }

    #[test]
    fn anything_else_is_known_by_the_id_the_app_registered() {
        assert_eq!(kde_component(&FakeEnv::default(), false).as_deref(), Some(HOST_APP_ID));
        // Not a Flatpak: a stray variable doesn't change what the app registered.
        let env = FakeEnv::with(&[("FLATPAK_ID", "org.example.Other")]);
        assert_eq!(kde_component(&env, false).as_deref(), Some(HOST_APP_ID));
    }

    #[test]
    fn a_flatpak_without_an_id_has_no_component_to_name() {
        assert_eq!(kde_component(&FakeEnv::default(), true), None);
    }

    // The format of the keys

    #[test]
    fn the_default_hotkeys_in_the_portals_format() {
        assert_eq!(trigger("Ctrl+Alt+Space").as_deref(), Some("CTRL+ALT+space"));
        assert_eq!(trigger("Ctrl+Alt+Backspace").as_deref(), Some("CTRL+ALT+BackSpace"));
        assert_eq!(trigger("Ctrl+Alt+Enter").as_deref(), Some("CTRL+ALT+Return"));
    }

    #[test]
    fn modifiers_come_in_a_fixed_order_whatever_the_spelling() {
        assert_eq!(trigger("Shift+Alt+Ctrl+R").as_deref(), Some("CTRL+ALT+SHIFT+r"));
        assert_eq!(trigger("alt + control + space").as_deref(), Some("CTRL+ALT+space"));
        assert_eq!(trigger("Super+F5").as_deref(), Some("LOGO+F5"));
    }

    #[test]
    fn letters_digits_function_keys_and_the_numpad() {
        assert_eq!(trigger("Ctrl+Alt+Z").as_deref(), Some("CTRL+ALT+z"));
        assert_eq!(trigger("Ctrl+Alt+7").as_deref(), Some("CTRL+ALT+7"));
        assert_eq!(trigger("Ctrl+F12").as_deref(), Some("CTRL+F12"));
        assert_eq!(trigger("Ctrl+Alt+Numpad5").as_deref(), Some("CTRL+ALT+KP_5"));
        assert_eq!(trigger("Ctrl+Alt+NumpadAdd").as_deref(), Some("CTRL+ALT+KP_Add"));
        assert_eq!(trigger("Ctrl+Alt+NumpadEnter").as_deref(), Some("CTRL+ALT+KP_Enter"));
    }

    #[test]
    fn navigation_and_punctuation_keys() {
        assert_eq!(trigger("Alt+PageUp").as_deref(), Some("ALT+Prior"));
        assert_eq!(trigger("Alt+PageDown").as_deref(), Some("ALT+Next"));
        assert_eq!(trigger("Ctrl+Alt+ArrowLeft").as_deref(), Some("CTRL+ALT+Left"));
        assert_eq!(trigger("Ctrl+Alt+Comma").as_deref(), Some("CTRL+ALT+comma"));
        assert_eq!(trigger("Ctrl+Alt+Quote").as_deref(), Some("CTRL+ALT+apostrophe"));
        assert_eq!(trigger("Ctrl+Alt+Backquote").as_deref(), Some("CTRL+ALT+grave"));
    }

    #[test]
    fn every_key_the_recorder_can_produce_has_a_name() {
        // The window's recorder offers these (`NAMED_KEYS` in hotkey.ts, letters, digits, function and numpad keys).
        let named = [
            "Space",
            "Enter",
            "Backspace",
            "Escape",
            "Delete",
            "Insert",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "Backquote",
            "Minus",
            "Equal",
            "Comma",
            "Period",
            "Slash",
            "Backslash",
            "Semicolon",
            "Quote",
            "BracketLeft",
            "BracketRight",
        ];
        let mut keys: Vec<String> = named.iter().map(|key| key.to_string()).collect();
        keys.extend(('A'..='Z').map(|c| c.to_string()));
        keys.extend((0..=9).map(|d| d.to_string()));
        keys.extend((1..=24).map(|n| format!("F{n}")));
        keys.extend((0..=9).map(|d| format!("Numpad{d}")));
        for pad in ["Add", "Subtract", "Multiply", "Divide", "Decimal", "Enter"] {
            keys.push(format!("Numpad{pad}"));
        }
        for key in keys {
            let accel = format!("Ctrl+Alt+{key}");
            let shortcut = parse_accel(&accel).unwrap_or_else(|err| panic!("{accel}: {err}"));
            assert!(
                xdg_trigger(&shortcut).is_some(),
                "{accel} has no name in the portal's format"
            );
        }
    }

    #[test]
    fn a_key_without_a_name_gives_no_preferred_key() {
        let shortcut = parse_accel("Ctrl+Alt+MediaStop").expect("the parser knows the key");
        assert_eq!(xdg_trigger(&shortcut), None);
    }

    // A backend with the portal thread played by the test

    struct Rig {
        manager: HotkeyManager,
        link: PortalLink,
        requests: Receiver<Request>,
        /// What the portal thread would bind first, when its session came up.
        first_bind: Vec<ShortcutRequest>,
        fired: Arc<Mutex<Vec<AppAction>>>,
        changes: Arc<AtomicUsize>,
    }

    /// The backend with nobody behind it, before the portal has been found.
    fn probing(bindings: &HotkeyBindings) -> Rig {
        let changes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&changes);
        let (backend, link, requests) = channel(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let fired = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&fired);
        let manager = HotkeyManager::new(
            Box::new(backend),
            Arc::new(move |action| sink.lock().unwrap().push(action)),
        );
        manager.apply(bindings);
        Rig {
            manager,
            link,
            requests,
            first_bind: Vec::new(),
            fired,
            changes,
        }
    }

    /// The backend once the portal thread has a session, with the first bind waiting for the user.
    fn rig_with(version: u32, bindings: &HotkeyBindings) -> Rig {
        let mut rig = probing(bindings);
        rig.first_bind = rig.link.session_up(version, None).expect("a bind");
        rig
    }

    fn rig() -> Rig {
        rig_with(2, &HotkeyBindings::default())
    }

    impl Rig {
        fn next_bind(&mut self) -> Vec<ShortcutRequest> {
            match self.requests.try_recv() {
                Ok(Request::Bind(shortcuts)) => shortcuts,
                other => panic!("expected a bind request, got {other:?}"),
            }
        }

        fn no_request(&mut self) {
            if let Ok(request) = self.requests.try_recv() {
                panic!("unexpected request {request:?}");
            }
        }

        fn fired(&self) -> Vec<AppAction> {
            self.fired.lock().unwrap().clone()
        }

        fn assigned_all(&self) {
            self.link.bound(&[
                assigned("replay", "Ctrl+Alt+Space"),
                assigned("stop", "Ctrl+Alt+Backspace"),
                assigned("catch-up", "Ctrl+Alt+Enter"),
            ]);
        }

        /// A second set of preferences, so that a bind is asked for again.
        fn change_preferences(&self) {
            self.manager.apply(&HotkeyBindings {
                replay: Some("Ctrl+Shift+R".into()),
                ..HotkeyBindings::default()
            });
        }
    }

    /// Runs `ask` while a stand-in for the portal thread answers the one settings request it makes with `answer`.
    fn answering_configure<T>(
        rig: &mut Rig,
        answer: Result<ConfigureOutcome, String>,
        ask: impl FnOnce(&Rig) -> T,
    ) -> T {
        let mut requests = std::mem::replace(&mut rig.requests, tauri::async_runtime::channel(1).1);
        let thread = std::thread::spawn(move || {
            match requests.blocking_recv() {
                Some(Request::Configure { reply, .. }) => reply.send(answer).unwrap(),
                other => panic!("expected a configure request, got {other:?}"),
            }
            requests
        });
        let outcome = ask(rig);
        rig.requests = thread.join().unwrap();
        outcome
    }

    fn assigned(id: &str, trigger: &str) -> Assigned {
        Assigned {
            id: id.into(),
            trigger: trigger.into(),
        }
    }

    // Asking the portal for the keys

    #[test]
    fn the_three_actions_are_bound_in_one_request_with_their_stored_keys_as_the_preference() {
        let rig = rig();
        assert_eq!(
            rig.first_bind,
            [
                ShortcutRequest {
                    id: "replay",
                    description: "Replay",
                    preferred_trigger: Some("CTRL+ALT+space".into())
                },
                ShortcutRequest {
                    id: "stop",
                    description: "Stop",
                    preferred_trigger: Some("CTRL+ALT+BackSpace".into())
                },
                ShortcutRequest {
                    id: "catch-up",
                    description: "Catch-up",
                    preferred_trigger: Some("CTRL+ALT+Return".into())
                },
            ]
        );
    }

    #[test]
    fn an_action_without_a_stored_key_is_still_bound_so_the_user_can_give_it_one() {
        let rig = rig_with(
            2,
            &HotkeyBindings {
                replay: Some("Ctrl+Alt+R".into()),
                stop: None,
                catch_up: Some("Ctrl+Alt+Enter".into()),
            },
        );
        let asked: Vec<_> = rig
            .first_bind
            .iter()
            .map(|shortcut| (shortcut.id, shortcut.preferred_trigger.as_deref()))
            .collect();
        assert_eq!(
            asked,
            [
                ("replay", Some("CTRL+ALT+r")),
                ("stop", None),
                ("catch-up", Some("CTRL+ALT+Return"))
            ]
        );
    }

    #[test]
    fn an_action_with_an_unreadable_stored_key_is_bound_without_a_preference() {
        let rig = rig_with(
            2,
            &HotkeyBindings {
                replay: Some("Ctrl+Frobnicate".into()),
                stop: Some("".into()),
                catch_up: None,
            },
        );
        assert_eq!(rig.first_bind.len(), 3);
        assert!(
            rig.first_bind
                .iter()
                .all(|shortcut| shortcut.preferred_trigger.is_none())
        );
    }

    #[test]
    fn a_key_with_no_name_is_asked_for_without_a_preference() {
        let rig = rig_with(
            2,
            &HotkeyBindings {
                replay: Some("Ctrl+Alt+MediaStop".into()),
                stop: None,
                catch_up: None,
            },
        );
        assert_eq!(rig.first_bind[0].preferred_trigger, None);
    }

    #[test]
    fn asking_for_the_same_again_does_not_ask_the_portal_again() {
        let mut rig = rig();
        rig.manager.apply(&HotkeyBindings::default());
        rig.no_request();
    }

    #[test]
    fn a_changed_preference_is_asked_for() {
        let mut rig = rig();
        rig.change_preferences();
        let bind = rig.next_bind();
        assert_eq!(bind.len(), 3);
        assert_eq!(bind[0].preferred_trigger.as_deref(), Some("CTRL+SHIFT+r"));
    }

    #[test]
    fn a_preference_that_changes_before_the_portal_is_there_is_bound_with_its_session() {
        let mut rig = probing(&HotkeyBindings::default());
        rig.change_preferences();
        // Nothing is queued for a portal nobody has found yet: the session binds what the state holds when it comes up.
        rig.no_request();
        let first = rig.link.session_up(2, None).expect("a bind");
        assert_eq!(first[0].preferred_trigger.as_deref(), Some("CTRL+SHIFT+r"));
    }

    // A session that starts with the shortcuts (KDE)

    #[test]
    fn plasma_sessions_start_with_the_shortcuts_it_kept() {
        // `XDG_CURRENT_DESKTOP` is a list, in any case.
        for desktop in ["KDE", "X-Custom:kde"] {
            let env = FakeEnv::with(&[("XDG_CURRENT_DESKTOP", desktop)]);
            assert!(sessions_keep_shortcuts(&env), "{desktop}");
        }
    }

    #[test]
    fn other_desktops_bind_in_every_session() {
        for desktop in ["GNOME", "ubuntu:GNOME", "Hyprland", "KDE-like", ""] {
            let env = FakeEnv::with(&[("XDG_CURRENT_DESKTOP", desktop)]);
            assert!(!sessions_keep_shortcuts(&env), "{desktop}");
        }
        assert!(!sessions_keep_shortcuts(&FakeEnv::default()));
    }

    fn kept_by_plasma() -> [Assigned; 3] {
        [
            assigned("replay", "Ctrl+Alt+Space"),
            assigned("stop", "Ctrl+Alt+Backspace"),
            assigned("catch-up", "Ctrl+Alt+Return"),
        ]
    }

    #[test]
    fn a_session_that_starts_with_every_shortcut_binds_none_and_has_their_keys() {
        let mut rig = probing(&HotkeyBindings::default());
        assert_eq!(rig.link.session_up(1, Some(&kept_by_plasma())), None);
        rig.no_request();
        let triggers = rig.manager.report().triggers;
        assert_eq!(triggers.replay.as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(triggers.catch_up.as_deref(), Some("Ctrl+Alt+Return"));
        assert_eq!(
            rig.manager.states(),
            HotkeyStates {
                replay: HotkeyState::Ok,
                stop: HotkeyState::Ok,
                catch_up: HotkeyState::Ok
            }
        );
        rig.link.activated("replay");
        assert_eq!(rig.fired(), [AppAction::Replay]);
    }

    #[test]
    fn the_same_keys_applied_again_do_not_bind_them_either() {
        let mut rig = probing(&HotkeyBindings::default());
        rig.link.session_up(1, Some(&kept_by_plasma()));
        rig.manager.apply(&HotkeyBindings::default());
        rig.no_request();
    }

    #[test]
    fn a_kept_shortcut_without_a_key_is_there_all_the_same() {
        // The user took the key away, or Plasma had none to give: binding again wouldn't give it one.
        let mut rig = probing(&HotkeyBindings::default());
        let mut kept = kept_by_plasma();
        kept[1] = assigned("stop", "");
        assert_eq!(rig.link.session_up(1, Some(&kept)), None);
        rig.no_request();
        assert_eq!(rig.manager.report().triggers.stop, None);
        assert_eq!(rig.manager.states().stop, HotkeyState::Unbound);
    }

    #[test]
    fn a_key_the_desktop_kept_counts_whatever_the_app_would_ask_for() {
        // The user changed it in System Settings, and the stored one is only a preference.
        let mut rig = probing(&HotkeyBindings {
            replay: Some("Ctrl+Shift+R".into()),
            ..HotkeyBindings::default()
        });
        assert_eq!(rig.link.session_up(1, Some(&kept_by_plasma())), None);
        rig.no_request();
        assert_eq!(rig.manager.report().triggers.replay.as_deref(), Some("Ctrl+Alt+Space"));
    }

    #[test]
    fn after_a_session_that_had_them_a_changed_preference_is_still_asked_for() {
        let mut rig = probing(&HotkeyBindings::default());
        rig.link.session_up(1, Some(&kept_by_plasma()));
        rig.change_preferences();
        assert_eq!(rig.next_bind()[0].preferred_trigger.as_deref(), Some("CTRL+SHIFT+r"));
    }

    #[test]
    fn a_session_that_misses_one_of_the_shortcuts_binds_all_of_them() {
        let rig = probing(&HotkeyBindings::default());
        let kept = kept_by_plasma();
        let bind = rig.link.session_up(1, Some(&kept[..2])).expect("a bind");
        assert_eq!(
            bind.iter().map(|shortcut| shortcut.id).collect::<Vec<_>>(),
            ["replay", "stop", "catch-up"]
        );
        // The bind waits for the user, as the first one always does.
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Busy));
    }

    #[test]
    fn shortcuts_the_app_does_not_have_count_for_nothing() {
        let rig = probing(&HotkeyBindings::default());
        let kept = [
            assigned("replay", "Ctrl+Alt+Space"),
            assigned("stop", "Ctrl+Alt+Backspace"),
            assigned("rewind", "Ctrl+Alt+R"),
        ];
        assert!(rig.link.session_up(1, Some(&kept)).is_some());
    }

    #[test]
    fn a_session_that_starts_with_no_shortcuts_binds() {
        let rig = probing(&HotkeyBindings::default());
        assert!(rig.link.session_up(1, Some(&[])).is_some());
    }

    #[test]
    fn with_the_shortcuts_there_the_settings_button_goes_to_where_they_are_changed() {
        // Not to a dialog: version 1 has no settings page to open, so the window opens the desktop's own.
        let rig = probing(&HotkeyBindings::default());
        rig.link.session_up(1, Some(&kept_by_plasma()));
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Unsupported));
    }

    // Keys going down and up

    #[test]
    fn a_pressed_shortcut_runs_its_action() {
        let rig = rig();
        rig.link.activated("replay");
        rig.link.activated("catch-up");
        rig.link.activated("stop");
        assert_eq!(rig.fired(), [AppAction::Replay, AppAction::CatchUp, AppAction::Stop]);
    }

    #[test]
    fn a_held_key_counts_once() {
        let rig = rig();
        rig.link.activated("replay");
        rig.link.activated("replay");
        rig.link.activated("replay");
        assert_eq!(rig.fired(), [AppAction::Replay]);
    }

    #[test]
    fn a_second_press_after_the_key_was_let_go_counts_again() {
        // The press that steps back one utterance during a replay.
        let rig = rig();
        rig.link.activated("replay");
        rig.link.deactivated("replay");
        rig.link.activated("replay");
        assert_eq!(rig.fired(), [AppAction::Replay, AppAction::Replay]);
    }

    #[test]
    fn letting_go_alone_runs_nothing_and_each_action_holds_on_its_own() {
        let rig = rig();
        rig.link.deactivated("replay");
        assert!(rig.fired().is_empty());
        rig.link.activated("replay");
        rig.link.activated("stop");
        rig.link.deactivated("stop");
        rig.link.activated("replay");
        assert_eq!(rig.fired(), [AppAction::Replay, AppAction::Stop]);
    }

    #[test]
    fn a_shortcut_the_app_never_asked_for_does_nothing() {
        let rig = rig();
        rig.link.activated("frobnicate");
        rig.link.deactivated("frobnicate");
        assert!(rig.fired().is_empty());
    }

    #[test]
    fn an_action_with_no_stored_key_runs_when_the_desktop_gives_it_one() {
        let rig = rig_with(
            2,
            &HotkeyBindings {
                replay: Some("Ctrl+Alt+R".into()),
                stop: None,
                catch_up: None,
            },
        );
        rig.link.activated("stop");
        assert_eq!(rig.fired(), [AppAction::Stop]);
    }

    // What the desktop gave

    #[test]
    fn the_keys_the_desktop_assigned_are_reported_and_the_actions_with_a_key_are_ok() {
        let rig = rig();
        rig.link.bound(&[
            assigned("replay", "Ctrl+Alt+Space"),
            assigned("catch-up", "Meta+Shift+C"),
            assigned("stop", ""),
        ]);
        let report = rig.manager.report();
        assert_eq!(report.backend, HotkeyBackendKind::Portal);
        assert_eq!(report.triggers.replay.as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(report.triggers.stop, None, "an empty description is no key");
        assert_eq!(report.triggers.catch_up.as_deref(), Some("Meta+Shift+C"));
        assert_eq!(
            rig.manager.states(),
            HotkeyStates {
                replay: HotkeyState::Ok,
                stop: HotkeyState::Unbound,
                catch_up: HotkeyState::Ok
            }
        );
    }

    #[test]
    fn before_the_desktop_answers_no_action_has_a_key() {
        let rig = rig();
        assert_eq!(rig.manager.report().triggers, HotkeyTriggers::default());
        assert_eq!(rig.manager.states(), HotkeyStates::UNBOUND);
    }

    #[test]
    fn while_the_portal_is_being_looked_for_the_hotkeys_read_as_the_portals_without_keys() {
        // Looking takes a moment at login; the warning that there are no hotkeys must not flash meanwhile.
        let rig = probing(&HotkeyBindings::default());
        assert_eq!(rig.manager.report().backend, HotkeyBackendKind::Portal);
        assert_eq!(rig.manager.report().triggers, HotkeyTriggers::default());
        assert_eq!(rig.manager.states(), HotkeyStates::UNBOUND);
    }

    #[test]
    fn an_action_the_desktop_leaves_out_of_its_answer_has_no_key() {
        let rig = rig();
        rig.assigned_all();
        rig.link.bound(&[assigned("replay", "Ctrl+Alt+Space")]);
        let triggers = rig.manager.report().triggers;
        assert_eq!(triggers.replay.as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(triggers.stop, None);
        assert_eq!(triggers.catch_up, None);
    }

    #[test]
    fn a_dismissed_dialog_leaves_the_actions_unbound_and_allows_asking_again() {
        let mut rig = rig();
        rig.link.bind_dismissed();
        assert_eq!(rig.manager.states(), HotkeyStates::UNBOUND);
        assert_eq!(rig.manager.report().backend, HotkeyBackendKind::Portal);
        assert_eq!(rig.manager.report().triggers, HotkeyTriggers::default());
        // The same request goes out again the next time the hotkeys are applied.
        rig.manager.apply(&HotkeyBindings::default());
        assert_eq!(rig.next_bind().len(), 3);
    }

    #[test]
    fn a_change_in_the_system_settings_updates_the_keys() {
        let rig = rig();
        rig.assigned_all();
        rig.link
            .changed(&[assigned("stop", "Meta+Escape"), assigned("unknown", "X")]);
        let triggers = rig.manager.report().triggers;
        assert_eq!(triggers.stop.as_deref(), Some("Meta+Escape"));
        assert_eq!(triggers.replay.as_deref(), Some("Ctrl+Alt+Space"), "the rest stays");
        // A key taken away.
        rig.link.changed(&[assigned("replay", "")]);
        assert_eq!(rig.manager.report().triggers.replay, None);
        assert_eq!(rig.manager.states().replay, HotkeyState::Unbound);
    }

    #[test]
    fn the_window_hears_of_every_change() {
        let rig = rig();
        let before = rig.changes.load(Ordering::SeqCst);
        rig.assigned_all();
        rig.link.changed(&[assigned("stop", "Meta+Escape")]);
        rig.link.bind_dismissed();
        rig.link.unavailable("gone".into());
        assert_eq!(rig.changes.load(Ordering::SeqCst), before + 4);
    }

    // A bind that fails, a portal that goes

    #[test]
    fn a_failed_bind_keeps_the_session_and_the_keys_it_had() {
        let mut rig = rig();
        rig.assigned_all();
        rig.change_preferences();
        rig.next_bind();

        rig.link.bind_failed("the compositor refused");

        let report = rig.manager.report();
        assert_eq!(report.backend, HotkeyBackendKind::Portal);
        assert_eq!(report.triggers.replay.as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(rig.manager.states().stop, HotkeyState::Ok);
        rig.link.activated("replay");
        assert_eq!(rig.fired(), [AppAction::Replay], "the keys still work");
    }

    #[test]
    fn a_failed_first_bind_leaves_the_actions_without_keys_but_the_portal_on_and_asking_again_possible() {
        let mut rig = rig();

        rig.link.bind_failed("the compositor refused");

        assert_eq!(rig.manager.report().backend, HotkeyBackendKind::Portal);
        assert_eq!(rig.manager.report().triggers, HotkeyTriggers::default());
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Opened));
        assert_eq!(rig.next_bind().len(), 3);
    }

    #[test]
    fn a_portal_that_goes_turns_the_hotkeys_off_and_says_so() {
        let rig = rig();
        rig.assigned_all();
        rig.link.unavailable("the portal went away".into());
        assert_eq!(rig.manager.report().backend, HotkeyBackendKind::None);
        assert_eq!(rig.manager.report().triggers, HotkeyTriggers::default());
        assert_eq!(rig.manager.states(), HotkeyStates::UNBOUND);
    }

    #[test]
    fn nothing_runs_while_the_portal_is_gone() {
        let rig = rig();
        rig.link.unavailable("the portal went away".into());
        rig.link.activated("replay");
        assert!(rig.fired().is_empty());
    }

    #[test]
    fn saying_that_the_portal_is_gone_again_is_no_news() {
        let rig = probing(&HotkeyBindings::default());
        rig.link.unavailable("not there".into());
        let after_first = rig.changes.load(Ordering::SeqCst);
        rig.link.unavailable("not there".into());
        assert_eq!(rig.changes.load(Ordering::SeqCst), after_first);
        rig.link.unavailable("not there either".into());
        assert_eq!(rig.changes.load(Ordering::SeqCst), after_first + 1);
    }

    #[test]
    fn a_portal_that_is_not_there_yet_is_looked_for_and_nothing_is_asked_of_it() {
        let mut rig = probing(&HotkeyBindings::default());
        rig.link.unavailable("not there".into());
        assert_eq!(rig.manager.report().backend, HotkeyBackendKind::None);
        rig.change_preferences();
        rig.no_request();
    }

    #[test]
    fn a_portal_that_comes_back_gets_what_is_wanted_then() {
        let rig = rig();
        rig.assigned_all();
        rig.link.unavailable("the session ended".into());
        rig.change_preferences();

        let again = rig.link.session_up(2, None).expect("a bind");

        assert_eq!(again[0].preferred_trigger.as_deref(), Some("CTRL+SHIFT+r"));
        assert_eq!(rig.manager.report().backend, HotkeyBackendKind::Portal);
        assert_eq!(
            rig.manager.report().triggers,
            HotkeyTriggers::default(),
            "its keys are asked for anew"
        );
        rig.link.activated("stop");
        assert_eq!(rig.fired(), [AppAction::Stop]);
    }

    #[test]
    fn a_bind_that_cannot_be_queued_is_not_left_pending() {
        let rig = rig();
        rig.link.bound(&[]);
        drop(rig.requests);
        rig.manager.apply(&HotkeyBindings {
            replay: Some("Ctrl+Shift+R".into()),
            ..HotkeyBindings::default()
        });
        // A bind that never went out must not keep the settings button saying "busy".
        assert_eq!(rig.manager.report().backend, HotkeyBackendKind::Portal);
        assert!(rig.manager.configure().is_err());
        assert_ne!(rig.manager.configure(), Ok(ConfigureOutcome::Busy));
    }

    // Settings

    #[test]
    fn the_window_never_switches_the_portal_hotkeys_off() {
        let mut rig = rig();
        assert!(!rig.manager.set_suspended(true));
        rig.no_request();
        rig.link.activated("replay");
        assert_eq!(rig.fired(), [AppAction::Replay]);
    }

    #[test]
    fn while_the_shortcuts_dialog_is_open_asking_for_the_settings_only_says_busy() {
        let mut rig = rig();
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Busy));
        // And nothing is queued behind the dialog to open a page long after.
        rig.no_request();
    }

    #[test]
    fn a_bind_the_user_has_not_answered_makes_a_later_one_busy_too() {
        let mut rig = rig();
        rig.link.bound(&[]);
        rig.change_preferences();
        rig.next_bind();
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Busy));
        rig.no_request();
    }

    #[test]
    fn while_the_portal_is_being_looked_for_asking_for_the_settings_says_busy() {
        let rig = probing(&HotkeyBindings::default());
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Busy));
    }

    #[test]
    fn after_a_dismissed_dialog_asking_for_the_settings_asks_for_the_shortcuts_again() {
        let mut rig = rig();
        rig.link.bind_dismissed();

        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Opened));
        let bind = rig.next_bind();
        assert_eq!(bind.len(), 3);
        assert_eq!(bind[0].preferred_trigger.as_deref(), Some("CTRL+ALT+space"));
        // The new dialog is open, so asking again waits for it, on version 1 as on version 2.
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Busy));
        rig.no_request();
    }

    #[test]
    fn version_one_has_no_settings_page_to_open_once_the_shortcuts_are_bound() {
        let mut rig = rig_with(1, &HotkeyBindings::default());
        rig.assigned_all();
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Unsupported));
        rig.no_request();
    }

    #[test]
    fn version_one_can_still_ask_for_the_shortcuts_again_after_a_dismissed_dialog() {
        let mut rig = rig_with(1, &HotkeyBindings::default());
        rig.link.bind_dismissed();
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Opened));
        assert_eq!(rig.next_bind().len(), 3);
    }

    #[test]
    fn version_two_asks_the_portal_to_show_its_settings_once_the_shortcuts_are_bound() {
        let mut rig = rig();
        rig.assigned_all();
        let outcome = answering_configure(&mut rig, Ok(ConfigureOutcome::Opened), |rig| rig.manager.configure());
        assert_eq!(outcome, Ok(ConfigureOutcome::Opened));
    }

    #[test]
    fn a_portal_that_cannot_configure_is_unsupported_and_an_error_is_passed_on() {
        let mut rig = rig();
        rig.assigned_all();
        let outcome = answering_configure(&mut rig, Ok(ConfigureOutcome::Unsupported), |rig| {
            rig.manager.configure()
        });
        assert_eq!(outcome, Ok(ConfigureOutcome::Unsupported));

        let outcome = answering_configure(&mut rig, Err("no window".to_string()), |rig| rig.manager.configure());
        assert_eq!(outcome, Err("no window".to_string()));
    }

    #[test]
    fn a_portal_that_is_gone_has_no_settings_page_either() {
        let mut rig = rig();
        rig.assigned_all();
        rig.link.unavailable("gone".into());
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Unsupported));
        rig.no_request();
    }

    /// The whole way, against a stand-in portal on a bus of its own: what goes out as D-Bus calls, and what comes back as
    /// signals.
    mod on_a_portal {
        use std::time::Instant;

        use super::*;
        use crate::fake_portal::{Asked, BindAnswer, FakePortal};
        use crate::private_bus::PrivateBus;

        /// Looks for a portal again soon, which most of these tests rely on to keep it short.
        const QUICK: &[Duration] = &[Duration::from_millis(50)];
        /// Not at all within a test, for the ones that look at the time the portal is gone.
        const NEVER: &[Duration] = &[Duration::from_secs(30)];

        struct World {
            manager: HotkeyManager,
            portal: Option<FakePortal>,
            fired: Arc<Mutex<Vec<AppAction>>>,
            changes: Arc<AtomicUsize>,
            // Last, so the daemon outlives everything that talks to it.
            bus: PrivateBus,
        }

        /// Starts the app's side of the hotkeys on a bus, with a portal of `version` on it (none for `None`), and applies
        /// `bindings`. `None` when there is no `dbus-daemon` to run the test on.
        fn world(
            version: Option<u32>,
            bindings: &HotkeyBindings,
            answer: BindAnswer,
            retry: &'static [Duration],
        ) -> Option<World> {
            world_on(version, bindings, answer, retry, None, |_| {})
        }

        /// [`world`], with `list_for` as in `start_on`, and the portal readied by `setup` before the app talks to it.
        fn world_on(
            version: Option<u32>,
            bindings: &HotkeyBindings,
            answer: BindAnswer,
            retry: &'static [Duration],
            list_for: Option<Duration>,
            setup: impl FnOnce(&FakePortal),
        ) -> Option<World> {
            let bus = PrivateBus::start()?;
            let portal = version.map(|version| FakePortal::start(&bus, version));
            if let Some(portal) = &portal {
                portal.answer_binds_with(answer);
                setup(portal);
            }
            let changes = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&changes);
            let address = bus.address.clone();
            let backend = start_on(
                move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                },
                async move { zbus::connection::Builder::address(address.as_str())?.build().await },
                retry,
                list_for,
            );
            let fired = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&fired);
            let manager = HotkeyManager::new(
                Box::new(backend),
                Arc::new(move |action| sink.lock().unwrap().push(action)),
            );
            manager.apply(bindings);
            Some(World {
                manager,
                portal,
                fired,
                changes,
                bus,
            })
        }

        fn standard(answer: BindAnswer) -> Option<World> {
            world(Some(2), &HotkeyBindings::default(), answer, NEVER)
        }

        /// Plasma: a version 1 portal whose sessions start with the shortcuts it keeps for the app, `kept`.
        fn plasma(kept: &[(&str, &str)], retry: &'static [Duration]) -> Option<World> {
            world_on(
                Some(1),
                &HotkeyBindings::default(),
                BindAnswer::Accept,
                retry,
                Some(LIST_TIMEOUT),
                |portal| portal.keep(kept),
            )
        }

        /// The three shortcuts as Plasma keeps them once they have been bound.
        const KEPT: &[(&str, &str)] = &[
            ("replay", "Ctrl+Alt+Space"),
            ("stop", "Ctrl+Alt+Backspace"),
            ("catch-up", "Ctrl+Alt+Return"),
        ];

        /// Polls until `done`, for up to ten seconds.
        fn until(what: &str, done: impl Fn() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done() {
                assert!(Instant::now() < deadline, "{what} within 10 s");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        impl World {
            fn portal(&self) -> &FakePortal {
                self.portal.as_ref().expect("a portal")
            }

            fn binds(&self) -> usize {
                self.portal().state.binds.lock().unwrap().len()
            }

            fn lists(&self) -> usize {
                self.portal().state.lists.load(Ordering::SeqCst)
            }

            fn wait_for_keys(&self) {
                until("the keys", || self.manager.report().triggers.replay.is_some());
            }

            fn backend(&self) -> HotkeyBackendKind {
                self.manager.report().backend
            }

            fn fired(&self) -> Vec<AppAction> {
                self.fired.lock().unwrap().clone()
            }
        }

        #[test]
        fn the_portal_is_asked_for_the_three_actions_and_hands_the_keys_back() {
            let Some(world) = standard(BindAnswer::Accept) else {
                return;
            };
            until("a bind request", || world.binds() > 0);
            assert_eq!(
                world.portal().state.binds.lock().unwrap().clone(),
                [vec![
                    Asked {
                        id: "replay".into(),
                        description: "Replay".into(),
                        preferred_trigger: Some("CTRL+ALT+space".into())
                    },
                    Asked {
                        id: "stop".into(),
                        description: "Stop".into(),
                        preferred_trigger: Some("CTRL+ALT+BackSpace".into())
                    },
                    Asked {
                        id: "catch-up".into(),
                        description: "Catch-up".into(),
                        preferred_trigger: Some("CTRL+ALT+Return".into())
                    },
                ]]
            );
            world.wait_for_keys();
            let report = world.manager.report();
            assert_eq!(report.backend, HotkeyBackendKind::Portal);
            assert_eq!(report.triggers.stop.as_deref(), Some("CTRL+ALT+BackSpace"));
            assert_eq!(report.triggers.catch_up.as_deref(), Some("CTRL+ALT+Return"));
            assert_eq!(
                world.manager.states(),
                HotkeyStates {
                    replay: HotkeyState::Ok,
                    stop: HotkeyState::Ok,
                    catch_up: HotkeyState::Ok
                }
            );
            assert!(world.changes.load(Ordering::SeqCst) >= 1, "the window is told");
        }

        #[test]
        fn an_action_with_no_stored_key_is_bound_all_the_same() {
            let bindings = HotkeyBindings {
                replay: Some("Ctrl+Alt+Space".into()),
                stop: None,
                catch_up: Some("not a key".into()),
            };
            let Some(world) = world(Some(2), &bindings, BindAnswer::Accept, NEVER) else {
                return;
            };
            until("a bind request", || world.binds() > 0);
            let asked = world.portal().state.binds.lock().unwrap()[0].clone();
            assert_eq!(
                asked.iter().map(|shortcut| shortcut.id.as_str()).collect::<Vec<_>>(),
                ["replay", "stop", "catch-up"]
            );
            assert_eq!(asked[1].preferred_trigger, None);
            assert_eq!(asked[2].preferred_trigger, None);
            // And the desktop's key for it works.
            world.wait_for_keys();
            world.portal().press("stop", true);
            until("the press", || !world.fired().is_empty());
            assert_eq!(world.fired(), [AppAction::Stop]);
        }

        #[test]
        fn a_press_runs_the_action_once_until_the_key_is_let_go() {
            let Some(world) = standard(BindAnswer::Accept) else {
                return;
            };
            world.wait_for_keys();
            world.portal().press("replay", true);
            world.portal().press("replay", true);
            world.portal().press("replay", false);
            world.portal().press("replay", true);
            world.portal().press("catch-up", true);
            until("the presses", || world.fired().len() >= 3);
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(
                world.fired(),
                [AppAction::Replay, AppAction::Replay, AppAction::CatchUp]
            );
        }

        #[test]
        fn keys_the_user_changes_in_the_system_settings_show_up() {
            let Some(world) = standard(BindAnswer::Accept) else {
                return;
            };
            world.wait_for_keys();
            world.portal().keys_changed(&[("stop", "Meta+Escape"), ("replay", "")]);
            until("the new keys", || {
                let triggers = world.manager.report().triggers;
                triggers.stop.as_deref() == Some("Meta+Escape") && triggers.replay.is_none()
            });
            assert_eq!(world.manager.states().replay, HotkeyState::Unbound);
            assert_eq!(world.manager.states().stop, HotkeyState::Ok);
        }

        #[test]
        fn a_dialog_the_user_closes_leaves_the_actions_without_keys_and_can_be_asked_for_again() {
            let Some(world) = standard(BindAnswer::Dismiss) else {
                return;
            };
            // The session came up (once), and the dialog was closed (twice).
            until("the window to be told", || world.changes.load(Ordering::SeqCst) >= 2);
            assert_eq!(world.backend(), HotkeyBackendKind::Portal);
            assert_eq!(world.manager.report().triggers, HotkeyTriggers::default());
            assert_eq!(world.manager.states(), HotkeyStates::UNBOUND);

            world.portal().answer_binds_with(BindAnswer::Accept);
            assert_eq!(world.manager.configure(), Ok(ConfigureOutcome::Opened));
            world.wait_for_keys();
            assert_eq!(world.binds(), 2);
        }

        #[test]
        fn a_portal_that_refuses_the_bind_keeps_the_hotkeys_on_and_can_be_asked_again() {
            let Some(world) = standard(BindAnswer::Fail) else {
                return;
            };
            until("the window to be told", || world.changes.load(Ordering::SeqCst) >= 2);
            assert_eq!(world.backend(), HotkeyBackendKind::Portal);
            assert_eq!(world.manager.states(), HotkeyStates::UNBOUND);

            world.portal().answer_binds_with(BindAnswer::Accept);
            assert_eq!(world.manager.configure(), Ok(ConfigureOutcome::Opened));
            world.wait_for_keys();
            assert_eq!(world.backend(), HotkeyBackendKind::Portal);
        }

        #[test]
        fn a_rebind_that_fails_keeps_the_session_and_the_keys() {
            let Some(world) = standard(BindAnswer::Accept) else {
                return;
            };
            world.wait_for_keys();
            world.portal().answer_binds_with(BindAnswer::Fail);

            world.manager.apply(&HotkeyBindings {
                replay: Some("Ctrl+Shift+R".into()),
                ..HotkeyBindings::default()
            });

            until("the second bind", || world.binds() >= 2);
            // Give the refusal time to arrive; the keys must still be there and work.
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(world.backend(), HotkeyBackendKind::Portal);
            assert!(world.manager.report().triggers.replay.is_some());
            world.portal().press("replay", true);
            until("the press", || !world.fired().is_empty());
        }

        #[test]
        fn the_settings_say_busy_while_the_dialog_is_open_and_open_the_page_after() {
            let Some(world) = standard(BindAnswer::Hold) else {
                return;
            };
            until("a bind request", || world.binds() > 0);

            assert_eq!(world.manager.configure(), Ok(ConfigureOutcome::Busy));
            assert_eq!(world.portal().state.configures.load(Ordering::SeqCst), 0);

            world.portal().release_held();
            world.wait_for_keys();
            assert_eq!(world.manager.configure(), Ok(ConfigureOutcome::Opened));
            assert_eq!(
                world.portal().state.configures.load(Ordering::SeqCst),
                1,
                "once, and not for the ask that was refused"
            );
        }

        #[test]
        fn a_session_the_desktop_ends_turns_the_hotkeys_off() {
            let Some(world) = world(Some(2), &HotkeyBindings::default(), BindAnswer::Accept, NEVER) else {
                return;
            };
            world.wait_for_keys();
            world.portal().close_session();
            until("the hotkeys to be off", || world.backend() == HotkeyBackendKind::None);
            assert_eq!(world.manager.report().triggers, HotkeyTriggers::default());
            world.portal().press("replay", true);
            std::thread::sleep(Duration::from_millis(100));
            assert!(world.fired().is_empty());
        }

        #[test]
        fn a_session_the_desktop_ended_is_made_again() {
            let Some(world) = world(Some(2), &HotkeyBindings::default(), BindAnswer::Accept, QUICK) else {
                return;
            };
            world.wait_for_keys();
            world.portal().close_session();
            until("the hotkeys to be off", || world.backend() == HotkeyBackendKind::None);

            until("the hotkeys to be back", || {
                world.backend() == HotkeyBackendKind::Portal && world.manager.report().triggers.replay.is_some()
            });
            assert_eq!(world.binds(), 2, "bound again in the new session");
            assert_eq!(
                world.lists(),
                0,
                "only KDE's portal is asked what a session starts with"
            );
            world.portal().press("stop", true);
            until("the press", || !world.fired().is_empty());
            assert_eq!(world.fired(), [AppAction::Stop]);
        }

        #[test]
        fn plasma_is_not_asked_again_for_shortcuts_it_kept_from_an_earlier_start() {
            // Plasma 6.3 answers every bind by opening System Settings on its Shortcuts page.
            let Some(world) = plasma(KEPT, NEVER) else {
                return;
            };
            world.wait_for_keys();
            assert_eq!(world.binds(), 0, "no bind, so no settings page at the start");
            assert_eq!(world.lists(), 1);
            let report = world.manager.report();
            assert_eq!(report.backend, HotkeyBackendKind::Portal);
            assert_eq!(report.triggers.stop.as_deref(), Some("Ctrl+Alt+Backspace"));
            assert_eq!(world.manager.states().catch_up, HotkeyState::Ok);
            world.portal().press("catch-up", true);
            until("the press", || !world.fired().is_empty());
            assert_eq!(world.fired(), [AppAction::CatchUp]);
        }

        #[test]
        fn plasma_is_asked_once_for_shortcuts_it_does_not_have_and_not_again_in_the_next_session() {
            let Some(world) = plasma(&[], QUICK) else {
                return;
            };
            world.wait_for_keys();
            assert_eq!(world.binds(), 1, "the first start sets them up");

            world.portal().close_session();
            until("the hotkeys to be off", || world.backend() == HotkeyBackendKind::None);
            until("the hotkeys to be back", || {
                world.backend() == HotkeyBackendKind::Portal && world.manager.report().triggers.replay.is_some()
            });
            assert_eq!(world.binds(), 1, "the new session found them kept");
            assert_eq!(world.lists(), 2);
            world.portal().press("stop", true);
            until("the press", || !world.fired().is_empty());
        }

        #[test]
        fn a_portal_that_does_not_list_the_shortcuts_in_time_gets_the_bind() {
            let limit = Duration::from_millis(300);
            let Some(world) = world_on(
                Some(1),
                &HotkeyBindings::default(),
                BindAnswer::Accept,
                NEVER,
                Some(limit),
                |portal| {
                    portal.keep(KEPT);
                    portal.hold_lists();
                },
            ) else {
                return;
            };
            let started = Instant::now();
            until("a bind request", || world.binds() > 0);
            assert!(started.elapsed() >= limit / 2, "bound before the list had its time");
            world.wait_for_keys();
            assert_eq!(world.lists(), 1);
            world.portal().press("replay", true);
            until("the press", || !world.fired().is_empty());
        }

        #[test]
        fn plasma_is_asked_for_all_three_when_it_kept_only_some() {
            // An action that came with an update, say.
            let Some(world) = plasma(&KEPT[..2], NEVER) else {
                return;
            };
            until("a bind request", || world.binds() > 0);
            let asked = world.portal().state.binds.lock().unwrap()[0].clone();
            assert_eq!(
                asked.iter().map(|shortcut| shortcut.id.as_str()).collect::<Vec<_>>(),
                ["replay", "stop", "catch-up"]
            );
            world.wait_for_keys();
        }

        #[test]
        fn no_portal_on_the_bus_turns_the_hotkeys_off() {
            let Some(world) = world(None, &HotkeyBindings::default(), BindAnswer::Accept, NEVER) else {
                return;
            };
            until("the hotkeys to be off", || world.backend() == HotkeyBackendKind::None);
        }

        #[test]
        fn a_portal_that_shows_up_later_gets_the_hotkeys() {
            let Some(world) = world(None, &HotkeyBindings::default(), BindAnswer::Accept, QUICK) else {
                return;
            };
            until("the hotkeys to be off", || world.backend() == HotkeyBackendKind::None);

            let portal = FakePortal::start(&world.bus, 2);

            until("the keys", || world.manager.report().triggers.replay.is_some());
            assert_eq!(world.backend(), HotkeyBackendKind::Portal);
            portal.press("replay", true);
            until("the press", || !world.fired().is_empty());
        }

        #[test]
        fn a_portal_without_shortcuts_is_not_one_until_its_shortcuts_backend_comes_up() {
            let Some(bus) = PrivateBus::start() else { return };
            let portal = FakePortal::start_without_shortcuts(&bus);
            let address = bus.address.clone();
            let manager = HotkeyManager::new(
                Box::new(start_on(
                    || {},
                    async move { zbus::connection::Builder::address(address.as_str())?.build().await },
                    QUICK,
                    None,
                )),
                Arc::new(|_| {}),
            );
            manager.apply(&HotkeyBindings::default());
            until("the hotkeys to be off", || {
                manager.report().backend == HotkeyBackendKind::None
            });
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(
                manager.report().backend,
                HotkeyBackendKind::None,
                "still no shortcuts interface"
            );

            portal.add_shortcuts(2);

            until("the keys", || manager.report().triggers.replay.is_some());
            assert_eq!(manager.report().backend, HotkeyBackendKind::Portal);
        }

        #[test]
        fn the_portal_is_not_looked_for_before_the_keys_are_registered() {
            // Otherwise the first bind could go out without them and the user would be asked twice.
            let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = Arc::clone(&connected);
            let manager = HotkeyManager::new(
                Box::new(start_on(
                    || {},
                    async move {
                        flag.store(true, Ordering::SeqCst);
                        zbus::connection::Builder::address("unix:path=/nonexistent/repeat-that-bus")?
                            .build()
                            .await
                    },
                    NEVER,
                    None,
                )),
                Arc::new(|_| {}),
            );
            std::thread::sleep(Duration::from_millis(200));
            assert!(
                !connected.load(Ordering::SeqCst),
                "looked for it before anything was registered"
            );

            manager.apply(&HotkeyBindings::default());

            until("the portal to be looked for", || connected.load(Ordering::SeqCst));
        }

        #[test]
        fn a_slow_portal_does_not_hold_up_the_start() {
            // `start_on` returns before the bus has even been connected to.
            let started = Instant::now();
            let manager = HotkeyManager::new(
                Box::new(start_on(
                    || {},
                    async {
                        std::thread::sleep(Duration::from_millis(500));
                        zbus::connection::Builder::address("unix:path=/nonexistent/repeat-that-bus")?
                            .build()
                            .await
                    },
                    NEVER,
                    None,
                )),
                Arc::new(|_| {}),
            );
            manager.apply(&HotkeyBindings::default());
            assert!(
                started.elapsed() < Duration::from_millis(400),
                "{:?}",
                started.elapsed()
            );
            assert_eq!(manager.report().backend, HotkeyBackendKind::Portal, "looking for it");
        }

        #[test]
        fn a_bus_that_cannot_be_reached_turns_the_hotkeys_off() {
            let manager = HotkeyManager::new(
                Box::new(start_on(
                    || {},
                    async {
                        zbus::connection::Builder::address("unix:path=/nonexistent/repeat-that-bus")?
                            .build()
                            .await
                    },
                    NEVER,
                    None,
                )),
                Arc::new(|_| {}),
            );
            manager.apply(&HotkeyBindings::default());
            until("the hotkeys to be off", || {
                manager.report().backend == HotkeyBackendKind::None
            });
        }

        #[test]
        fn version_two_opens_the_systems_shortcut_settings() {
            let Some(world) = standard(BindAnswer::Accept) else {
                return;
            };
            world.wait_for_keys();
            assert_eq!(world.manager.configure(), Ok(ConfigureOutcome::Opened));
            assert_eq!(world.portal().state.configures.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn version_one_is_not_asked_to() {
            let Some(world) = world(Some(1), &HotkeyBindings::default(), BindAnswer::Accept, NEVER) else {
                return;
            };
            world.wait_for_keys();
            assert_eq!(world.manager.configure(), Ok(ConfigureOutcome::Unsupported));
            assert_eq!(world.portal().state.configures.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn a_request_for_the_settings_that_is_too_old_to_answer_is_dropped_not_run_later() {
            let Some(bus) = PrivateBus::start() else { return };
            let portal = FakePortal::start(&bus, 2);
            let (backend, link, requests) = channel(|| {});
            let address = bus.address.clone();
            tauri::async_runtime::spawn(async move {
                let connection = zbus::connection::Builder::address(address.as_str())
                    .unwrap()
                    .build()
                    .await
                    .unwrap();
                worker::run(&link, requests, connection, QUICK, None).await;
            });
            until("a bind", || !portal.state.binds.lock().unwrap().is_empty());

            // Nobody is waiting for this one any more.
            let (reply, stale) = mpsc::channel();
            backend
                .requests
                .try_send(Request::Configure {
                    reply,
                    deadline: Instant::now(),
                })
                .unwrap();
            // A live one right behind it is answered, so the stale one has been looked at by then.
            let (reply, live) = mpsc::channel();
            backend
                .requests
                .try_send(Request::Configure {
                    reply,
                    deadline: Instant::now() + Duration::from_secs(10),
                })
                .unwrap();

            assert_eq!(
                live.recv_timeout(Duration::from_secs(10)),
                Ok(Ok(ConfigureOutcome::Opened))
            );
            assert_eq!(portal.state.configures.load(Ordering::SeqCst), 1, "only the live one");
            assert!(stale.try_recv().is_err());
        }
    }
}
