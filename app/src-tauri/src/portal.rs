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

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::async_runtime::{Receiver, Sender};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

use crate::actions::AppAction;
use crate::hotkeys::{EdgeHandler, KeyEdge, ShortcutBackend};
use crate::ipc::{ConfigureOutcome, HOTKEY_COUNT, HotkeyBackendKind, HotkeyTriggers};
use crate::locking::lock;
use crate::settings::{HOTKEY_ACTIONS, HOTKEYS, parse_accel};

/// The ID the app tells the portal it has: the name of its desktop file without `.desktop`. Packages install
/// `me.roboroads.repeat-that.desktop`, so the portal can show the app's name and icon next to its shortcuts.
pub const HOST_APP_ID: &str = "me.roboroads.repeat-that";

/// How long to wait for the portal to answer a request to open its settings.
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// Has the desktop show where the keys are changed. The answer goes back through the channel.
    Configure(mpsc::Sender<Result<ConfigureOutcome, String>>),
}

#[derive(Default)]
struct State {
    handlers: [Option<Arc<EdgeHandler>>; HOTKEY_COUNT],
    /// The key each action was registered with, in the portal's format.
    preferred: [Option<String>; HOTKEY_COUNT],
    /// The key the portal says each action has.
    assigned: [Option<String>; HOTKEY_COUNT],
    /// What the last `Bind` asked for, so asking for the same again doesn't.
    sent: Option<Vec<ShortcutRequest>>,
    /// Why the portal no longer works; the hotkeys are gone then.
    failure: Option<String>,
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
    /// The version of the GlobalShortcuts interface.
    version: u32,
}

/// What the portal thread tells the backend.
#[derive(Clone)]
pub struct PortalLink(Arc<Shared>);

/// A backend that has no thread behind it yet, with the link to feed it and the requests it makes. `on_change` is called
/// whenever what the window shows about the keys may have changed.
pub fn channel(
    version: u32,
    on_change: impl Fn() + Send + Sync + 'static,
) -> (PortalBackend, PortalLink, Receiver<Request>) {
    let shared = Arc::new(Shared {
        state: Mutex::default(),
        on_change: Box::new(on_change),
    });
    let (requests, receiver) = tauri::async_runtime::channel(QUEUE);
    let backend = PortalBackend {
        shared: Arc::clone(&shared),
        requests,
        version,
    };
    (backend, PortalLink(shared), receiver)
}

/// Starts talking to the portal. `version` is what the session check found out.
pub fn start(version: u32, on_change: impl Fn() + Send + Sync + 'static) -> PortalBackend {
    start_on(version, on_change, zbus::Connection::session())
}

/// [`start`] on the bus `connect` reaches.
fn start_on(
    version: u32,
    on_change: impl Fn() + Send + Sync + 'static,
    connect: impl Future<Output = zbus::Result<zbus::Connection>> + Send + 'static,
) -> PortalBackend {
    let (backend, link, requests) = channel(version, on_change);
    tauri::async_runtime::spawn(async move {
        let outcome = match connect.await {
            Ok(connection) => worker::run(&link, requests, connection).await,
            Err(err) => Err(format!("could not reach the session bus: {err}")),
        };
        if let Err(reason) = outcome {
            link.fail(reason);
        }
    });
    backend
}

impl ShortcutBackend for PortalBackend {
    fn register(&self, action: AppAction, accel: &str, on_edge: EdgeHandler) -> Result<(), String> {
        let slot = slot_of_action(action).ok_or_else(|| format!("{action:?} has no hotkey"))?;
        let preferred = parse_accel(accel).ok().and_then(|shortcut| xdg_trigger(&shortcut));
        let mut state = lock(&self.shared.state);
        state.handlers[slot] = Some(Arc::new(on_edge));
        state.preferred[slot] = preferred;
        Ok(())
    }

    fn unregister(&self, action: AppAction, _accel: &str) {
        let Some(slot) = slot_of_action(action) else { return };
        let mut state = lock(&self.shared.state);
        state.handlers[slot] = None;
        state.preferred[slot] = None;
    }

    fn commit(&self) {
        let requests: Vec<ShortcutRequest> = {
            let mut state = lock(&self.shared.state);
            if state.failure.is_some() {
                return;
            }
            let requests: Vec<ShortcutRequest> = HOTKEYS
                .iter()
                .enumerate()
                .filter(|(slot, _)| state.handlers[*slot].is_some())
                .map(|(slot, def)| ShortcutRequest {
                    id: def.command,
                    description: def.label,
                    preferred_trigger: state.preferred[slot].clone(),
                })
                .collect();
            if requests.is_empty() || state.sent.as_ref() == Some(&requests) {
                return;
            }
            state.sent = Some(requests.clone());
            requests
        };
        if let Err(err) = self.requests.try_send(Request::Bind(requests)) {
            self.shared.fail(format!("the portal thread is not listening: {err}"));
        }
    }

    fn kind(&self) -> HotkeyBackendKind {
        if lock(&self.shared.state).failure.is_some() {
            HotkeyBackendKind::None
        } else {
            HotkeyBackendKind::Portal
        }
    }

    fn triggers(&self) -> HotkeyTriggers {
        let state = lock(&self.shared.state);
        let mut triggers = HotkeyTriggers::default();
        for ((slot, trigger), handler) in triggers
            .slots_mut()
            .into_iter()
            .zip(&state.assigned)
            .zip(&state.handlers)
        {
            if handler.is_some() {
                slot.clone_from(trigger);
            }
        }
        triggers
    }

    /// The window has no recorder under the portal, so there is nothing to switch the hotkeys off for.
    fn can_suspend(&self) -> bool {
        false
    }

    fn configure(&self) -> Result<ConfigureOutcome, String> {
        // `ConfigureShortcuts` came with version 2 of the interface; asking an older portal would only fail.
        if self.version < 2 || lock(&self.shared.state).failure.is_some() {
            return Ok(ConfigureOutcome::Unsupported);
        }
        let (reply, answer) = mpsc::channel();
        self.requests
            .try_send(Request::Configure(reply))
            .map_err(|err| format!("the portal thread is not listening: {err}"))?;
        answer
            .recv_timeout(CONFIGURE_TIMEOUT)
            .map_err(|_| "the desktop did not answer".to_string())?
    }
}

impl Shared {
    /// Runs the handler of `id`'s action with `edge`. A shortcut nobody registered, and every shortcut once the portal
    /// has failed, does nothing.
    fn edge(&self, id: &str, edge: KeyEdge) {
        let Some(slot) = slot_of_id(id) else { return };
        let handler = {
            let state = lock(&self.state);
            if state.failure.is_some() {
                return;
            }
            state.handlers[slot].clone()
        };
        // Outside the lock: the action may do anything, including registering hotkeys.
        if let Some(handler) = handler {
            handler(edge);
        }
    }

    fn fail(&self, reason: String) {
        eprintln!("hotkeys through the portal stopped: {reason}");
        {
            let mut state = lock(&self.state);
            state.failure = Some(reason);
            state.assigned = Default::default();
        }
        (self.on_change)();
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

    /// The portal answered a bind with the shortcuts that are bound now.
    pub fn bound(&self, shortcuts: &[Assigned]) {
        self.0.assign(shortcuts, true);
    }

    /// The user closed the dialog without choosing. The actions have no keys, and asking again is allowed.
    pub fn bind_dismissed(&self) {
        lock(&self.0.state).sent = None;
        (self.0.on_change)();
    }

    /// The portal says some shortcuts changed, usually because the user changed them in the system settings.
    pub fn changed(&self, shortcuts: &[Assigned]) {
        self.0.assign(shortcuts, false);
    }

    /// The portal can't be used (anymore): the hotkeys are gone, and the window shows the commands instead.
    pub fn fail(&self, reason: String) {
        self.0.fail(reason);
    }
}

mod worker {
    //! The thread that talks to the portal: connects, creates the session, binds what the backend asks for and tells
    //! the link what happens. Not unit tested, since it needs a portal; `PortalLink` is the part with the rules.

    use ashpd::desktop::ResponseError;
    use ashpd::desktop::global_shortcuts::{
        BindShortcutsOptions, ConfigureShortcutsOptions, GlobalShortcuts, NewShortcut, Shortcut,
    };
    use ashpd::{AppID, Error};
    use futures_util::future::{Either, select};
    use futures_util::stream::{self, Stream, StreamExt};

    use super::*;

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

    /// Runs until the session ends, which is a failure: the app has no other way to end it.
    pub async fn run(
        link: &PortalLink,
        mut requests: Receiver<Request>,
        connection: zbus::Connection,
    ) -> Result<(), String> {
        // Portals know apps by their ID. A packaged app is told apart by its desktop file, an unpackaged one has to say
        // who it is. Older portals don't have the call; they find the app by other means.
        match AppID::try_from(HOST_APP_ID) {
            Ok(id) => {
                if let Err(err) = ashpd::register_host_app_with_connection(connection.clone(), id).await {
                    eprintln!("could not tell the portal the app's ID (fine on older portals): {err}");
                }
            }
            Err(err) => eprintln!("{HOST_APP_ID} is not a valid app ID: {err}"),
        }

        let portal = GlobalShortcuts::with_connection(connection)
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

        loop {
            let request = std::pin::pin!(requests.recv());
            let signal = std::pin::pin!(signals.next());
            match select(request, signal).await {
                // The backend is gone with the app.
                Either::Left((None, _)) => return Ok(()),
                Either::Left((Some(Request::Bind(shortcuts)), _)) => {
                    let new: Vec<NewShortcut> = shortcuts
                        .iter()
                        .map(|shortcut| {
                            NewShortcut::new(shortcut.id, shortcut.description)
                                .preferred_trigger(shortcut.preferred_trigger.as_deref())
                        })
                        .collect();
                    // Waits for the user if the desktop asks them.
                    let answer = portal
                        .bind_shortcuts(&session, &new, None, BindShortcutsOptions::default())
                        .await
                        .and_then(|request| request.response());
                    match answer {
                        Ok(bound) => link.bound(&assigned(bound.shortcuts())),
                        Err(Error::Response(ResponseError::Cancelled)) => link.bind_dismissed(),
                        Err(err) => return Err(format!("could not bind the shortcuts: {err}")),
                    }
                }
                Either::Left((Some(Request::Configure(reply)), _)) => {
                    let answer = match portal
                        .configure_shortcuts(&session, None, ConfigureShortcutsOptions::default())
                        .await
                    {
                        Ok(()) => Ok(ConfigureOutcome::Opened),
                        Err(Error::RequiresVersion(..)) => Ok(ConfigureOutcome::Unsupported),
                        Err(err) => Err(err.to_string()),
                    };
                    // The asker may have given up waiting.
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
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::hotkeys::HotkeyManager;
    use crate::ipc::{HotkeyBindings, HotkeyState, HotkeyStates};

    fn trigger(accel: &str) -> Option<String> {
        xdg_trigger(&parse_accel(accel).unwrap())
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
        fired: Arc<Mutex<Vec<AppAction>>>,
        changes: Arc<AtomicUsize>,
    }

    fn rig_with(version: u32, bindings: &HotkeyBindings) -> Rig {
        let changes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&changes);
        let (backend, link, requests) = channel(version, move || {
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
            fired,
            changes,
        }
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
        let mut rig = rig();
        assert_eq!(
            rig.next_bind(),
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
        rig.no_request();
    }

    #[test]
    fn an_action_without_a_stored_key_is_left_out() {
        let mut rig = rig_with(
            2,
            &HotkeyBindings {
                replay: Some("Ctrl+Alt+R".into()),
                stop: None,
                catch_up: Some("Ctrl+Alt+Enter".into()),
            },
        );
        let ids: Vec<_> = rig.next_bind().iter().map(|shortcut| shortcut.id).collect();
        assert_eq!(ids, ["replay", "catch-up"]);
    }

    #[test]
    fn asking_for_the_same_again_does_not_ask_the_portal_again() {
        let mut rig = rig();
        rig.next_bind();
        rig.manager.apply(&HotkeyBindings::default());
        rig.no_request();
    }

    #[test]
    fn a_changed_preference_is_asked_for() {
        let mut rig = rig();
        rig.next_bind();
        rig.manager.apply(&HotkeyBindings {
            replay: Some("Ctrl+Shift+R".into()),
            ..HotkeyBindings::default()
        });
        let bind = rig.next_bind();
        assert_eq!(bind[0].preferred_trigger.as_deref(), Some("CTRL+SHIFT+r"));
    }

    #[test]
    fn a_key_with_no_name_is_asked_for_without_a_preference() {
        let mut rig = rig_with(
            2,
            &HotkeyBindings {
                replay: Some("Ctrl+Alt+MediaStop".into()),
                stop: None,
                catch_up: None,
            },
        );
        assert_eq!(rig.next_bind()[0].preferred_trigger, None);
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
    fn an_unbound_action_does_nothing_even_if_the_desktop_has_a_key_for_it() {
        let rig = rig_with(
            2,
            &HotkeyBindings {
                replay: Some("Ctrl+Alt+R".into()),
                stop: None,
                catch_up: None,
            },
        );
        rig.link.activated("stop");
        assert!(rig.fired().is_empty());
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
        rig.next_bind();
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
        rig.link.fail("gone".into());
        assert_eq!(rig.changes.load(Ordering::SeqCst), before + 4);
    }

    // The portal fails

    #[test]
    fn a_portal_error_turns_the_hotkeys_off_and_says_so() {
        let rig = rig();
        rig.assigned_all();
        rig.link.fail("the portal went away".into());
        assert_eq!(rig.manager.report().backend, HotkeyBackendKind::None);
        assert_eq!(rig.manager.report().triggers, HotkeyTriggers::default());
        assert_eq!(rig.manager.states(), HotkeyStates::UNBOUND);
    }

    #[test]
    fn nothing_runs_after_the_portal_failed() {
        let rig = rig();
        rig.link.fail("the portal went away".into());
        rig.link.activated("replay");
        assert!(rig.fired().is_empty());
    }

    #[test]
    fn nothing_is_asked_of_a_failed_portal() {
        let mut rig = rig();
        rig.next_bind();
        rig.link.fail("the portal went away".into());
        rig.manager.apply(&HotkeyBindings {
            replay: Some("Ctrl+Shift+R".into()),
            ..HotkeyBindings::default()
        });
        rig.no_request();
    }

    #[test]
    fn a_thread_that_stopped_listening_is_a_failure() {
        let rig = rig();
        drop(rig.requests);
        rig.manager.apply(&HotkeyBindings {
            replay: Some("Ctrl+Shift+R".into()),
            ..HotkeyBindings::default()
        });
        assert_eq!(rig.manager.report().backend, HotkeyBackendKind::None);
    }

    // Settings

    #[test]
    fn the_window_never_switches_the_portal_hotkeys_off() {
        let mut rig = rig();
        rig.next_bind();
        assert!(!rig.manager.set_suspended(true));
        rig.no_request();
        rig.link.activated("replay");
        assert_eq!(rig.fired(), [AppAction::Replay]);
    }

    #[test]
    fn version_one_has_no_settings_page_to_open() {
        let mut rig = rig_with(1, &HotkeyBindings::default());
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Unsupported));
        rig.next_bind();
        rig.no_request();
    }

    #[test]
    fn version_two_asks_the_portal_to_show_its_settings() {
        let mut rig = rig();
        rig.next_bind();
        let mut requests = rig.requests;
        let thread = std::thread::spawn(move || match requests.blocking_recv() {
            Some(Request::Configure(reply)) => reply.send(Ok(ConfigureOutcome::Opened)).unwrap(),
            other => panic!("expected a configure request, got {other:?}"),
        });
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Opened));
        thread.join().unwrap();
    }

    #[test]
    fn a_portal_that_cannot_configure_is_unsupported_and_an_error_is_passed_on() {
        let mut rig = rig();
        rig.next_bind();
        let mut requests = rig.requests;
        let thread = std::thread::spawn(move || {
            for answer in [Ok(ConfigureOutcome::Unsupported), Err("no window".to_string())] {
                match requests.blocking_recv() {
                    Some(Request::Configure(reply)) => reply.send(answer).unwrap(),
                    other => panic!("expected a configure request, got {other:?}"),
                }
            }
        });
        assert_eq!(rig.manager.configure(), Ok(ConfigureOutcome::Unsupported));
        assert_eq!(rig.manager.configure(), Err("no window".to_string()));
        thread.join().unwrap();
    }

    #[test]
    fn a_failed_portal_has_no_settings_page_either() {
        let mut rig = rig();
        rig.next_bind();
        rig.link.fail("gone".into());
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

        struct World {
            manager: HotkeyManager,
            portal: Option<FakePortal>,
            fired: Arc<Mutex<Vec<AppAction>>>,
            changes: Arc<AtomicUsize>,
            // Last, so the daemon outlives everything that talks to it.
            _bus: PrivateBus,
        }

        /// Starts the app's side of the hotkeys on a bus, with a portal of `version` on it (none for `None`), and applies
        /// `bindings`. `None` when there is no `dbus-daemon` to run the test on.
        fn world(version: Option<u32>, bindings: &HotkeyBindings, answer: BindAnswer) -> Option<World> {
            let bus = PrivateBus::start()?;
            let portal = version.map(|version| FakePortal::start(&bus, version));
            if let Some(portal) = &portal {
                portal.answer_binds_with(answer);
            }
            let changes = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&changes);
            let address = bus.address.clone();
            let backend = start_on(
                version.unwrap_or(2),
                move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                },
                async move { zbus::connection::Builder::address(address.as_str())?.build().await },
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
                _bus: bus,
            })
        }

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

            fn wait_for_keys(&self) {
                until("the keys", || self.manager.report().triggers.replay.is_some());
            }

            fn fired(&self) -> Vec<AppAction> {
                self.fired.lock().unwrap().clone()
            }
        }

        #[test]
        fn the_portal_is_asked_for_the_three_actions_and_hands_the_keys_back() {
            let Some(world) = world(Some(2), &HotkeyBindings::default(), BindAnswer::Accept) else {
                return;
            };
            until("a bind request", || {
                !world.portal().state.binds.lock().unwrap().is_empty()
            });
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
        fn a_press_runs_the_action_once_until_the_key_is_let_go() {
            let Some(world) = world(Some(2), &HotkeyBindings::default(), BindAnswer::Accept) else {
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
            let Some(world) = world(Some(2), &HotkeyBindings::default(), BindAnswer::Accept) else {
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
        fn a_dialog_the_user_closes_leaves_the_actions_without_keys() {
            let Some(world) = world(Some(2), &HotkeyBindings::default(), BindAnswer::Dismiss) else {
                return;
            };
            until("the window to be told", || world.changes.load(Ordering::SeqCst) >= 1);
            assert_eq!(world.manager.report().backend, HotkeyBackendKind::Portal);
            assert_eq!(world.manager.report().triggers, HotkeyTriggers::default());
            assert_eq!(world.manager.states(), HotkeyStates::UNBOUND);
        }

        #[test]
        fn a_portal_that_refuses_the_bind_turns_the_hotkeys_off() {
            let Some(world) = world(Some(2), &HotkeyBindings::default(), BindAnswer::Fail) else {
                return;
            };
            until("the hotkeys to be off", || {
                world.manager.report().backend == HotkeyBackendKind::None
            });
            assert_eq!(world.manager.states(), HotkeyStates::UNBOUND);
        }

        #[test]
        fn a_session_the_desktop_ends_turns_the_hotkeys_off() {
            let Some(world) = world(Some(2), &HotkeyBindings::default(), BindAnswer::Accept) else {
                return;
            };
            world.wait_for_keys();
            world.portal().close_session();
            until("the hotkeys to be off", || {
                world.manager.report().backend == HotkeyBackendKind::None
            });
            assert_eq!(world.manager.report().triggers, HotkeyTriggers::default());
            world.portal().press("replay", true);
            std::thread::sleep(Duration::from_millis(100));
            assert!(world.fired().is_empty());
        }

        #[test]
        fn no_portal_on_the_bus_turns_the_hotkeys_off() {
            let Some(world) = world(None, &HotkeyBindings::default(), BindAnswer::Accept) else {
                return;
            };
            until("the hotkeys to be off", || {
                world.manager.report().backend == HotkeyBackendKind::None
            });
        }

        #[test]
        fn a_bus_that_cannot_be_reached_turns_the_hotkeys_off() {
            let manager = HotkeyManager::new(
                Box::new(start_on(2, || {}, async {
                    zbus::connection::Builder::address("unix:path=/nonexistent/repeat-that-bus")?
                        .build()
                        .await
                })),
                Arc::new(|_| {}),
            );
            manager.apply(&HotkeyBindings::default());
            until("the hotkeys to be off", || {
                manager.report().backend == HotkeyBackendKind::None
            });
        }

        #[test]
        fn version_two_opens_the_systems_shortcut_settings() {
            let Some(world) = world(Some(2), &HotkeyBindings::default(), BindAnswer::Accept) else {
                return;
            };
            world.wait_for_keys();
            assert_eq!(world.manager.configure(), Ok(ConfigureOutcome::Opened));
            assert_eq!(world.portal().state.configures.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn version_one_is_not_asked_to() {
            let Some(world) = world(Some(1), &HotkeyBindings::default(), BindAnswer::Accept) else {
                return;
            };
            world.wait_for_keys();
            assert_eq!(world.manager.configure(), Ok(ConfigureOutcome::Unsupported));
            assert_eq!(world.portal().state.configures.load(Ordering::SeqCst), 0);
        }
    }
}
