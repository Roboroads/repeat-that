//! Global hotkeys: which combination is registered for which action, whether the OS accepted it, and the rule that a
//! held key counts once.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::actions::AppAction;
use crate::ipc::{ConfigureOutcome, HotkeyBackendKind, HotkeyBindings, HotkeyState, HotkeyStates, HotkeyTriggers};
use crate::locking::lock;
use crate::settings::{HOTKEY_ACTIONS, parse_accel};

/// One edge of a key combination, as the OS reports it. Holding a key sends `Pressed` over and over (key repeat) and a
/// single `Released` at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEdge {
    Pressed,
    Released,
}

/// Called for every press and release of one registered combination, from whichever thread the OS reports on.
pub type EdgeHandler = Box<dyn Fn(KeyEdge) + Send + Sync>;

/// Registers key combinations with the OS. The real ones are the global-shortcut plugin and the desktop's shortcut
/// portal; tests use a fake.
pub trait ShortcutBackend: Send + Sync {
    /// Registers `accel` for `action` and calls `on_edge` for every press and release. An error means the OS refused,
    /// usually because another app holds the combination.
    fn register(&self, action: AppAction, accel: &str, on_edge: EdgeHandler) -> Result<(), String>;
    /// Best effort: a combination that is already gone has nothing to unregister.
    fn unregister(&self, action: AppAction, accel: &str);
    /// A round of `register` and `unregister` calls is over. A backend that has to ask the OS for all its keys at once
    /// (the portal shows one dialog for them) does that here.
    fn commit(&self) {}
    /// How the hotkeys work: see [`HotkeyBackendKind`]. A backend can give up on its own, and then says `None`.
    fn kind(&self) -> HotkeyBackendKind {
        HotkeyBackendKind::Plugin
    }
    /// The key the OS assigned to each action, for a backend where the OS decides.
    fn triggers(&self) -> HotkeyTriggers {
        HotkeyTriggers::default()
    }
    /// Whether the hotkeys can be switched off for a moment (while the window records a combination). Where the user
    /// records nothing in the window, there is nothing to switch off.
    fn can_suspend(&self) -> bool {
        true
    }
    /// Asks the OS to show where the user changes the keys.
    fn configure(&self) -> Result<ConfigureOutcome, String> {
        Ok(ConfigureOutcome::Unsupported)
    }
}

/// [`ShortcutBackend`] on `tauri-plugin-global-shortcut`.
pub struct PluginBackend<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> ShortcutBackend for PluginBackend<R> {
    fn register(&self, _action: AppAction, accel: &str, on_edge: EdgeHandler) -> Result<(), String> {
        self.0
            .global_shortcut()
            .on_shortcut(accel, move |_app, _shortcut, event| {
                on_edge(match event.state {
                    ShortcutState::Pressed => KeyEdge::Pressed,
                    ShortcutState::Released => KeyEdge::Released,
                });
            })
            .map_err(|err| err.to_string())
    }

    fn unregister(&self, _action: AppAction, accel: &str) {
        // Best effort, as documented on the trait.
        let _ = self.0.global_shortcut().unregister(accel);
    }
}

/// [`ShortcutBackend`] for a desktop with no way to have global hotkeys. It registers nothing; the command-line actions
/// are what the user binds instead.
pub struct NoBackend;

impl ShortcutBackend for NoBackend {
    fn register(&self, _action: AppAction, _accel: &str, _on_edge: EdgeHandler) -> Result<(), String> {
        Ok(())
    }

    fn unregister(&self, _action: AppAction, _accel: &str) {}

    fn kind(&self) -> HotkeyBackendKind {
        HotkeyBackendKind::None
    }
}

/// Decides which key events start an action: the first `Pressed` of each action does, repeats don't until `Released`.
#[derive(Debug, Default)]
pub struct PressTracker {
    held: [bool; HOTKEY_ACTIONS.len()],
}

impl PressTracker {
    /// Whether this edge should run `action`.
    pub fn on_edge(&mut self, action: AppAction, edge: KeyEdge) -> bool {
        let Some(slot) = HOTKEY_ACTIONS.iter().position(|&a| a == action) else {
            return false;
        };
        match edge {
            KeyEdge::Pressed => !std::mem::replace(&mut self.held[slot], true),
            KeyEdge::Released => {
                self.held[slot] = false;
                false
            }
        }
    }

    /// Forgets that `action`'s key is held, for when its registration is replaced under it.
    pub fn release(&mut self, action: AppAction) {
        if let Some(slot) = HOTKEY_ACTIONS.iter().position(|&a| a == action) {
            self.held[slot] = false;
        }
    }
}

impl HotkeyStates {
    pub const UNBOUND: HotkeyStates = HotkeyStates {
        replay: HotkeyState::Unbound,
        stop: HotkeyState::Unbound,
        catch_up: HotkeyState::Unbound,
    };
}

/// Nothing registered.
impl Default for HotkeyStates {
    fn default() -> Self {
        HotkeyStates::UNBOUND
    }
}

/// One hotkey action's place in the manager.
struct Slot {
    /// The combination registered with the OS right now, and the spelling it was registered under.
    live: Option<(Shortcut, String)>,
    /// The combination that `state` was found for. A suspended manager keeps the last real answer for it.
    tried: Option<Shortcut>,
    state: HotkeyState,
}

impl Slot {
    fn new() -> Self {
        Slot {
            live: None,
            tried: None,
            state: HotkeyState::Unbound,
        }
    }
}

/// What the settings ask of one action's hotkey.
enum Wanted {
    Unbound,
    /// An accelerator that can't be understood.
    Invalid,
    /// A combination, with the accelerator as it is spelled.
    Bound(Shortcut, String),
}

struct Inner {
    desired: HotkeyBindings,
    slots: [Slot; HOTKEY_ACTIONS.len()],
    suspended: bool,
}

/// How the hotkeys work, for the status.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HotkeyReport {
    pub backend: HotkeyBackendKind,
    /// The keys the OS assigned, under the portal.
    pub triggers: HotkeyTriggers,
}

/// Keeps the OS registrations in line with the settings and runs the action of a pressed hotkey.
pub struct HotkeyManager {
    backend: Box<dyn ShortcutBackend>,
    dispatch: Arc<dyn Fn(AppAction) + Send + Sync>,
    tracker: Arc<Mutex<PressTracker>>,
    inner: Mutex<Inner>,
}

impl HotkeyManager {
    /// Registers nothing yet; call [`apply`](Self::apply).
    pub fn new(backend: Box<dyn ShortcutBackend>, dispatch: Arc<dyn Fn(AppAction) + Send + Sync>) -> Self {
        HotkeyManager {
            backend,
            dispatch,
            tracker: Arc::default(),
            inner: Mutex::new(Inner {
                desired: HotkeyBindings {
                    replay: None,
                    stop: None,
                    catch_up: None,
                },
                slots: std::array::from_fn(|_| Slot::new()),
                suspended: false,
            }),
        }
    }

    /// Makes `bindings` the registered hotkeys. Only the combinations that changed are unregistered and registered, the
    /// others stay as they are (so nothing else can grab them in between); combinations another app held are tried
    /// again. While suspended it only remembers them.
    ///
    /// Registering hops to the main thread and waits for it, so don't call this from a thread the main thread waits on.
    pub fn apply(&self, bindings: &HotkeyBindings) {
        let mut inner = lock(&self.inner);
        inner.desired = bindings.clone();
        self.reconcile(&mut inner);
    }

    /// Unregisters every hotkey (`true`, while the window records a combination) or registers the wanted ones again
    /// (`false`, which also retries the ones another app held). Returns whether that changed anything.
    pub fn set_suspended(&self, suspended: bool) -> bool {
        if !self.backend.can_suspend() {
            return false;
        }
        let mut inner = lock(&self.inner);
        if inner.suspended == suspended {
            return false;
        }
        inner.suspended = suspended;
        self.reconcile(&mut inner);
        true
    }

    /// Where each hotkey stands. Where the OS decides the keys (the portal) a registered action is `Ok` once the OS
    /// has given it a key and `Unbound` until then, and with no hotkeys at all every action is `Unbound`.
    pub fn states(&self) -> HotkeyStates {
        let inner = lock(&self.inner);
        let kind = self.backend.kind();
        let mut states = HotkeyStates::UNBOUND;
        if kind == HotkeyBackendKind::None {
            return states;
        }
        let assigned = self.backend.triggers();
        for ((state, slot), assigned) in states.slots_mut().into_iter().zip(&inner.slots).zip(assigned.slots()) {
            *state = match kind {
                HotkeyBackendKind::Portal if slot.live.is_some() => {
                    if assigned.is_some() {
                        HotkeyState::Ok
                    } else {
                        HotkeyState::Unbound
                    }
                }
                _ => slot.state,
            };
        }
        states
    }

    /// How the hotkeys work now, and the keys the OS assigned.
    pub fn report(&self) -> HotkeyReport {
        let backend = self.backend.kind();
        HotkeyReport {
            backend,
            triggers: if backend == HotkeyBackendKind::Portal {
                self.backend.triggers()
            } else {
                HotkeyTriggers::default()
            },
        }
    }

    /// Asks the OS to show where the user changes the keys, see [`ShortcutBackend::configure`].
    pub fn configure(&self) -> Result<ConfigureOutcome, String> {
        self.backend.configure()
    }

    /// Brings the OS registrations to `inner.desired`, touching only what differs.
    fn reconcile(&self, inner: &mut Inner) {
        let Inner {
            desired,
            slots,
            suspended,
        } = inner;
        let wanted: Vec<Wanted> = HOTKEY_ACTIONS
            .iter()
            .map(|&action| match desired.accel(action) {
                None => Wanted::Unbound,
                Some(accel) => match parse_accel(accel) {
                    Ok(shortcut) => Wanted::Bound(shortcut, accel.trim().to_string()),
                    Err(_) => Wanted::Invalid,
                },
            })
            .collect();

        // First let go of what is no longer wanted, so a combination can move from one action to another.
        for ((&action, slot), wanted) in HOTKEY_ACTIONS.iter().zip(slots.iter_mut()).zip(&wanted) {
            let still_wanted = match (&slot.live, wanted) {
                (Some((live, _)), Wanted::Bound(shortcut, _)) => !*suspended && live == shortcut,
                _ => false,
            };
            if !still_wanted && let Some((_, accel)) = slot.live.take() {
                self.backend.unregister(action, &accel);
                lock(&self.tracker).release(action);
            }
        }

        for ((&action, slot), wanted) in HOTKEY_ACTIONS.iter().zip(slots.iter_mut()).zip(&wanted) {
            match wanted {
                Wanted::Unbound => (slot.state, slot.tried) = (HotkeyState::Unbound, None),
                Wanted::Invalid => (slot.state, slot.tried) = (HotkeyState::Invalid, None),
                Wanted::Bound(shortcut, _) if slot.live.is_some() => {
                    (slot.state, slot.tried) = (HotkeyState::Ok, Some(*shortcut))
                }
                // Nothing is registered while suspended. A combination that was found out before keeps its answer
                // (a real "in use" must not turn into "ok"), one that was just chosen has none yet.
                Wanted::Bound(shortcut, _) if *suspended => {
                    if slot.tried != Some(*shortcut) {
                        (slot.state, slot.tried) = (HotkeyState::Ok, Some(*shortcut));
                    }
                }
                Wanted::Bound(shortcut, accel) => {
                    slot.tried = Some(*shortcut);
                    match self.backend.register(action, accel, self.edge_handler(action)) {
                        Ok(()) => {
                            slot.live = Some((*shortcut, accel.clone()));
                            slot.state = HotkeyState::Ok;
                        }
                        Err(err) => {
                            eprintln!("could not register the {action:?} hotkey {accel}: {err}");
                            slot.state = HotkeyState::InUse;
                        }
                    }
                }
            }
        }
        self.backend.commit();
    }

    fn edge_handler(&self, action: AppAction) -> EdgeHandler {
        let tracker = Arc::clone(&self.tracker);
        let dispatch = Arc::clone(&self.dispatch);
        Box::new(move |edge| {
            // The lock is released before the action runs: it may do anything, including touching the hotkeys.
            let run = lock(&tracker).on_edge(action, edge);
            if run {
                dispatch(action);
            }
        })
    }
}

/// A backend that registers into memory, for tests here and of the settings service.
#[cfg(test)]
pub mod fake {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    #[derive(Default)]
    pub struct FakeBackend {
        pub log: Mutex<Vec<String>>,
        pub handlers: Mutex<Vec<(String, EdgeHandler)>>,
        pub refused: Mutex<HashSet<String>>,
        /// The actions the manager registered, in order, whatever their combination.
        pub actions: Mutex<Vec<AppAction>>,
        /// How many rounds of registrations were over (`commit` calls).
        pub commits: AtomicUsize,
        /// What the backend says it is; `Plugin` unless a test changes it.
        pub kind: Mutex<HotkeyBackendKind>,
        pub triggers: Mutex<HotkeyTriggers>,
        pub keeps_hotkeys_on: AtomicBool,
        pub configure_answer: Mutex<Option<Result<ConfigureOutcome, String>>>,
    }

    impl FakeBackend {
        pub fn set_kind(&self, kind: HotkeyBackendKind) {
            *self.kind.lock().unwrap() = kind;
        }

        pub fn set_triggers(&self, replay: Option<&str>, stop: Option<&str>, catch_up: Option<&str>) {
            *self.triggers.lock().unwrap() = HotkeyTriggers {
                replay: replay.map(String::from),
                stop: stop.map(String::from),
                catch_up: catch_up.map(String::from),
            };
        }

        pub fn commits(&self) -> usize {
            self.commits.load(Ordering::SeqCst)
        }

        pub fn refuse(&self, accel: &str) {
            self.refused.lock().unwrap().insert(accel.to_string());
        }

        pub fn allow(&self, accel: &str) {
            self.refused.lock().unwrap().remove(accel);
        }

        pub fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        pub fn clear_log(&self) {
            self.log.lock().unwrap().clear();
        }

        pub fn registered(&self) -> Vec<String> {
            self.handlers.lock().unwrap().iter().map(|(a, _)| a.clone()).collect()
        }

        /// Like [`registered`](Self::registered), in a fixed order: which registration happened first doesn't matter.
        pub fn registered_sorted(&self) -> Vec<String> {
            let mut registered = self.registered();
            registered.sort();
            registered
        }

        pub fn send(&self, accel: &str, edge: KeyEdge) {
            let handlers = self.handlers.lock().unwrap();
            let (_, handler) = handlers
                .iter()
                .find(|(a, _)| a == accel)
                .unwrap_or_else(|| panic!("{accel} is not registered"));
            handler(edge);
        }
    }

    /// The manager owns its backend; a test keeps a second handle on it.
    impl ShortcutBackend for Arc<FakeBackend> {
        fn register(&self, action: AppAction, accel: &str, on_edge: EdgeHandler) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("register {accel}"));
            self.actions.lock().unwrap().push(action);
            if self.refused.lock().unwrap().contains(accel) {
                return Err("already taken".into());
            }
            let mut handlers = self.handlers.lock().unwrap();
            if handlers.iter().any(|(a, _)| a == accel) {
                return Err("registered twice".into());
            }
            handlers.push((accel.to_string(), on_edge));
            Ok(())
        }

        fn unregister(&self, _action: AppAction, accel: &str) {
            self.log.lock().unwrap().push(format!("unregister {accel}"));
            self.handlers.lock().unwrap().retain(|(a, _)| a != accel);
        }

        fn commit(&self) {
            self.commits.fetch_add(1, Ordering::SeqCst);
        }

        fn kind(&self) -> HotkeyBackendKind {
            *self.kind.lock().unwrap()
        }

        fn triggers(&self) -> HotkeyTriggers {
            self.triggers.lock().unwrap().clone()
        }

        fn can_suspend(&self) -> bool {
            !self.keeps_hotkeys_on.load(Ordering::SeqCst)
        }

        fn configure(&self) -> Result<ConfigureOutcome, String> {
            self.configure_answer
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Ok(ConfigureOutcome::Unsupported))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeBackend;
    use super::*;

    struct Setup {
        manager: HotkeyManager,
        backend: Arc<FakeBackend>,
        fired: Arc<Mutex<Vec<AppAction>>>,
    }

    fn setup() -> Setup {
        let backend = Arc::new(FakeBackend::default());
        let fired = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&fired);
        let manager = HotkeyManager::new(
            Box::new(Arc::clone(&backend)),
            Arc::new(move |action| sink.lock().unwrap().push(action)),
        );
        Setup {
            manager,
            backend,
            fired,
        }
    }

    fn bindings(replay: Option<&str>, stop: Option<&str>, catch_up: Option<&str>) -> HotkeyBindings {
        HotkeyBindings {
            replay: replay.map(String::from),
            stop: stop.map(String::from),
            catch_up: catch_up.map(String::from),
        }
    }

    fn states(replay: HotkeyState, stop: HotkeyState, catch_up: HotkeyState) -> HotkeyStates {
        HotkeyStates { replay, stop, catch_up }
    }

    const SPACE: &str = "Ctrl+Alt+Space";
    const BACKSPACE: &str = "Ctrl+Alt+Backspace";
    const ENTER: &str = "Ctrl+Alt+Enter";

    // PressTracker

    #[test]
    fn the_first_press_counts_and_repeats_do_not() {
        let mut tracker = PressTracker::default();
        assert!(tracker.on_edge(AppAction::Replay, KeyEdge::Pressed));
        assert!(!tracker.on_edge(AppAction::Replay, KeyEdge::Pressed));
        assert!(!tracker.on_edge(AppAction::Replay, KeyEdge::Pressed));
    }

    #[test]
    fn a_release_lets_the_next_press_count() {
        let mut tracker = PressTracker::default();
        assert!(tracker.on_edge(AppAction::Stop, KeyEdge::Pressed));
        assert!(!tracker.on_edge(AppAction::Stop, KeyEdge::Released));
        assert!(tracker.on_edge(AppAction::Stop, KeyEdge::Pressed));
    }

    #[test]
    fn a_release_never_runs_an_action() {
        let mut tracker = PressTracker::default();
        assert!(!tracker.on_edge(AppAction::CatchUp, KeyEdge::Released));
        assert!(!tracker.on_edge(AppAction::CatchUp, KeyEdge::Released));
        assert!(tracker.on_edge(AppAction::CatchUp, KeyEdge::Pressed));
    }

    #[test]
    fn each_action_holds_on_its_own() {
        let mut tracker = PressTracker::default();
        assert!(tracker.on_edge(AppAction::Replay, KeyEdge::Pressed));
        assert!(tracker.on_edge(AppAction::Stop, KeyEdge::Pressed));
        assert!(!tracker.on_edge(AppAction::Replay, KeyEdge::Pressed));
        tracker.on_edge(AppAction::Replay, KeyEdge::Released);
        assert!(
            !tracker.on_edge(AppAction::Stop, KeyEdge::Pressed),
            "stop is still held"
        );
        assert!(tracker.on_edge(AppAction::Replay, KeyEdge::Pressed));
    }

    #[test]
    fn release_forgets_the_held_key_of_one_action() {
        let mut tracker = PressTracker::default();
        tracker.on_edge(AppAction::Replay, KeyEdge::Pressed);
        tracker.on_edge(AppAction::Stop, KeyEdge::Pressed);
        tracker.release(AppAction::Replay);
        assert!(tracker.on_edge(AppAction::Replay, KeyEdge::Pressed));
        assert!(
            !tracker.on_edge(AppAction::Stop, KeyEdge::Pressed),
            "stop is still held"
        );
    }

    #[test]
    fn actions_without_a_hotkey_never_fire() {
        let mut tracker = PressTracker::default();
        for action in [AppAction::Quit, AppAction::OpenSettings, AppAction::TogglePause] {
            assert!(!tracker.on_edge(action, KeyEdge::Pressed), "{action:?}");
        }
    }

    // HotkeyManager

    #[test]
    fn nothing_is_registered_before_apply() {
        let s = setup();
        assert!(s.backend.log().is_empty());
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Unbound, HotkeyState::Unbound, HotkeyState::Unbound)
        );
    }

    #[test]
    fn the_default_hotkeys_register_and_are_ok() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        assert_eq!(s.backend.registered(), [SPACE, BACKSPACE, ENTER]);
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Ok, HotkeyState::Ok)
        );
    }

    #[test]
    fn an_unbound_action_registers_nothing() {
        let s = setup();
        s.manager.apply(&bindings(Some(SPACE), None, None));
        assert_eq!(s.backend.registered(), [SPACE]);
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Unbound, HotkeyState::Unbound)
        );
    }

    #[test]
    fn a_combination_the_os_refuses_is_in_use_and_the_rest_still_work() {
        let s = setup();
        s.backend.refuse(BACKSPACE);
        s.manager.apply(&HotkeyBindings::default());
        assert_eq!(s.backend.registered(), [SPACE, ENTER]);
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::InUse, HotkeyState::Ok)
        );
    }

    #[test]
    fn an_accelerator_that_cannot_be_parsed_is_invalid_and_never_registered() {
        let s = setup();
        s.manager
            .apply(&bindings(Some("Ctrl+Frobnicate"), Some(BACKSPACE), None));
        assert_eq!(s.backend.log(), [format!("register {BACKSPACE}")]);
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Invalid, HotkeyState::Ok, HotkeyState::Unbound)
        );
    }

    #[test]
    fn a_press_runs_the_action_once_until_released() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.backend.send(SPACE, KeyEdge::Pressed);
        s.backend.send(SPACE, KeyEdge::Pressed);
        s.backend.send(SPACE, KeyEdge::Pressed);
        assert_eq!(*s.fired.lock().unwrap(), [AppAction::Replay]);
        s.backend.send(SPACE, KeyEdge::Released);
        s.backend.send(SPACE, KeyEdge::Pressed);
        assert_eq!(*s.fired.lock().unwrap(), [AppAction::Replay, AppAction::Replay]);
    }

    #[test]
    fn each_hotkey_runs_its_own_action() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.backend.send(ENTER, KeyEdge::Pressed);
        s.backend.send(BACKSPACE, KeyEdge::Pressed);
        s.backend.send(SPACE, KeyEdge::Pressed);
        assert_eq!(
            *s.fired.lock().unwrap(),
            [AppAction::CatchUp, AppAction::Stop, AppAction::Replay]
        );
    }

    #[test]
    fn a_release_alone_runs_nothing() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.backend.send(SPACE, KeyEdge::Released);
        assert!(s.fired.lock().unwrap().is_empty());
    }

    #[test]
    fn applying_new_bindings_replaces_the_old_registrations() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.backend.clear_log();
        s.manager.apply(&bindings(Some("Ctrl+Shift+R"), Some(BACKSPACE), None));
        assert_eq!(s.backend.registered_sorted(), [BACKSPACE, "Ctrl+Shift+R"]);
        assert!(s.backend.log().contains(&format!("unregister {SPACE}")));
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Ok, HotkeyState::Unbound)
        );
        // The old combination is dead, the new one works.
        s.backend.send("Ctrl+Shift+R", KeyEdge::Pressed);
        assert_eq!(*s.fired.lock().unwrap(), [AppAction::Replay]);
    }

    #[test]
    fn combinations_that_did_not_change_are_never_released() {
        // Between an unregister and the next register another app could take the combination.
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.backend.clear_log();
        s.manager
            .apply(&bindings(Some("Ctrl+Shift+R"), Some(BACKSPACE), Some(ENTER)));
        let log = s.backend.log();
        assert_eq!(
            log,
            [format!("unregister {SPACE}"), "register Ctrl+Shift+R".to_string()]
        );
    }

    #[test]
    fn a_swap_leaves_the_third_combination_alone() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.backend.clear_log();
        s.manager.apply(&bindings(Some(BACKSPACE), Some(SPACE), Some(ENTER)));
        assert!(
            s.backend.log().iter().all(|entry| !entry.ends_with(ENTER)),
            "{:?}",
            s.backend.log()
        );
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Ok, HotkeyState::Ok)
        );
    }

    #[test]
    fn the_same_combination_in_another_spelling_is_left_alone() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.backend.clear_log();
        s.manager
            .apply(&bindings(Some("alt + CTRL + space"), Some(BACKSPACE), Some(ENTER)));
        assert!(s.backend.log().is_empty(), "{:?}", s.backend.log());
    }

    #[test]
    fn a_held_key_stays_held_when_only_another_action_changes() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.backend.send(SPACE, KeyEdge::Pressed);
        s.manager
            .apply(&bindings(Some(SPACE), Some("Ctrl+Shift+S"), Some(ENTER)));
        s.backend.send(SPACE, KeyEdge::Pressed); // key repeat
        assert_eq!(*s.fired.lock().unwrap(), [AppAction::Replay]);
    }

    #[test]
    fn a_combination_another_app_held_is_tried_again_on_the_next_apply() {
        let s = setup();
        s.backend.refuse(SPACE);
        s.manager.apply(&HotkeyBindings::default());
        assert_eq!(s.manager.states().replay, HotkeyState::InUse);
        s.backend.allow(SPACE);
        s.manager.apply(&HotkeyBindings::default());
        assert_eq!(s.manager.states().replay, HotkeyState::Ok);
    }

    #[test]
    fn two_actions_can_swap_combinations() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.manager.apply(&bindings(Some(BACKSPACE), Some(SPACE), Some(ENTER)));
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Ok, HotkeyState::Ok)
        );
        s.backend.send(SPACE, KeyEdge::Pressed);
        assert_eq!(*s.fired.lock().unwrap(), [AppAction::Stop]);
    }

    #[test]
    fn suspending_unregisters_everything_and_resuming_registers_it_again() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.manager.set_suspended(true);
        assert!(s.backend.registered().is_empty());
        s.manager.set_suspended(false);
        assert_eq!(s.backend.registered(), [SPACE, BACKSPACE, ENTER]);
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Ok, HotkeyState::Ok)
        );
    }

    #[test]
    fn suspending_twice_or_resuming_unsuspended_does_no_work() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.manager.set_suspended(false);
        s.backend.clear_log();
        s.manager.set_suspended(false);
        assert!(s.backend.log().is_empty());
        s.manager.set_suspended(true);
        s.backend.clear_log();
        s.manager.set_suspended(true);
        assert!(s.backend.log().is_empty());
    }

    #[test]
    fn bindings_applied_while_suspended_register_on_resume() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.manager.set_suspended(true);
        s.manager.apply(&bindings(Some("Ctrl+Shift+R"), None, Some("garbage+")));
        assert!(s.backend.registered().is_empty());
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Unbound, HotkeyState::Invalid),
            "provisional states while suspended"
        );
        s.manager.set_suspended(false);
        assert_eq!(s.backend.registered(), ["Ctrl+Shift+R"]);
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Unbound, HotkeyState::Invalid)
        );
    }

    #[test]
    fn suspending_keeps_the_last_real_state() {
        let s = setup();
        s.backend.refuse(BACKSPACE);
        s.manager.apply(&HotkeyBindings::default());
        s.manager.set_suspended(true);
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::InUse, HotkeyState::Ok),
            "a combination another app holds is not reported as fine just because it is not registered right now"
        );
        s.manager.set_suspended(false);
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::InUse, HotkeyState::Ok)
        );
        s.backend.allow(BACKSPACE);
        s.manager.set_suspended(true);
        s.manager.set_suspended(false);
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Ok, HotkeyState::Ok)
        );
    }

    #[test]
    fn a_combination_changed_while_suspended_has_no_real_state_yet() {
        let s = setup();
        s.backend.refuse(BACKSPACE);
        s.manager.apply(&HotkeyBindings::default());
        s.manager.set_suspended(true);
        s.manager
            .apply(&bindings(Some(SPACE), Some("Ctrl+Shift+S"), Some(ENTER)));
        assert_eq!(s.manager.states().stop, HotkeyState::Ok);
        s.manager.set_suspended(false);
        assert_eq!(s.manager.states().stop, HotkeyState::Ok);
    }

    #[test]
    fn resuming_retries_a_combination_that_was_in_use() {
        let s = setup();
        s.backend.refuse(SPACE);
        s.manager.apply(&HotkeyBindings::default());
        assert_eq!(s.manager.states().replay, HotkeyState::InUse);
        s.backend.allow(SPACE);
        s.manager.set_suspended(true);
        s.manager.set_suspended(false);
        assert_eq!(s.manager.states().replay, HotkeyState::Ok);
    }

    #[test]
    fn a_key_held_across_a_re_registration_does_not_stay_stuck() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.backend.send(SPACE, KeyEdge::Pressed);
        s.manager.set_suspended(true);
        s.manager.set_suspended(false);
        s.backend.send(SPACE, KeyEdge::Pressed);
        assert_eq!(*s.fired.lock().unwrap(), [AppAction::Replay, AppAction::Replay]);
    }

    #[test]
    fn states_serialize_as_the_contract_names_them() {
        let json = serde_json::to_value(states(HotkeyState::Ok, HotkeyState::InUse, HotkeyState::Invalid)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "replay": "ok", "stop": "inUse", "catchUp": "invalid" })
        );
        assert_eq!(serde_json::to_value(HotkeyState::Unbound).unwrap(), "unbound");
    }

    // Which backend, and what it does with the manager

    #[test]
    fn each_action_is_registered_under_its_own_name() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        assert_eq!(
            *s.backend.actions.lock().unwrap(),
            [AppAction::Replay, AppAction::Stop, AppAction::CatchUp]
        );
    }

    #[test]
    fn a_round_of_registrations_is_committed_once() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        assert_eq!(s.backend.commits(), 1);
        s.manager
            .apply(&bindings(Some("Ctrl+Shift+R"), Some(BACKSPACE), Some(ENTER)));
        assert_eq!(s.backend.commits(), 2);
    }

    #[test]
    fn the_report_names_the_backend_and_the_keys_the_os_assigned() {
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        assert_eq!(
            s.manager.report(),
            HotkeyReport {
                backend: HotkeyBackendKind::Plugin,
                triggers: HotkeyTriggers::default()
            }
        );

        s.backend.set_kind(HotkeyBackendKind::Portal);
        s.backend
            .set_triggers(Some("Ctrl+Alt+Space"), None, Some("Ctrl+Alt+Enter"));
        let report = s.manager.report();
        assert_eq!(report.backend, HotkeyBackendKind::Portal);
        assert_eq!(report.triggers.replay.as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(report.triggers.stop, None);
        assert_eq!(report.triggers.catch_up.as_deref(), Some("Ctrl+Alt+Enter"));
    }

    #[test]
    fn the_plugin_never_reports_triggers() {
        // Only the portal assigns keys; whatever a plugin backend might say, the window shows its own settings.
        let s = setup();
        s.manager.apply(&HotkeyBindings::default());
        s.backend.set_triggers(Some("Ctrl+Alt+Space"), None, None);
        assert_eq!(s.manager.report().triggers, HotkeyTriggers::default());
    }

    #[test]
    fn under_the_portal_a_key_the_os_assigned_is_ok_and_one_it_did_not_is_unbound() {
        let s = setup();
        s.backend.set_kind(HotkeyBackendKind::Portal);
        s.manager.apply(&HotkeyBindings::default());
        s.backend
            .set_triggers(Some("Ctrl+Alt+Space"), None, Some("Ctrl+Alt+Enter"));
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Unbound, HotkeyState::Ok)
        );
    }

    #[test]
    fn without_hotkeys_every_state_is_unbound() {
        let s = setup();
        s.backend.set_kind(HotkeyBackendKind::None);
        s.manager.apply(&HotkeyBindings::default());
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Unbound, HotkeyState::Unbound, HotkeyState::Unbound)
        );
        assert_eq!(s.manager.report().backend, HotkeyBackendKind::None);
    }

    #[test]
    fn a_backend_that_gives_up_later_turns_the_states_unbound() {
        let s = setup();
        s.backend.set_kind(HotkeyBackendKind::Portal);
        s.manager.apply(&HotkeyBindings::default());
        s.backend.set_triggers(
            Some("Ctrl+Alt+Space"),
            Some("Ctrl+Alt+Backspace"),
            Some("Ctrl+Alt+Enter"),
        );
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Ok, HotkeyState::Ok, HotkeyState::Ok)
        );
        s.backend.set_kind(HotkeyBackendKind::None);
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Unbound, HotkeyState::Unbound, HotkeyState::Unbound)
        );
    }

    #[test]
    fn invalid_and_unbound_settings_keep_their_state_under_the_portal() {
        let s = setup();
        s.backend.set_kind(HotkeyBackendKind::Portal);
        s.manager.apply(&bindings(Some("Ctrl+Frobnicate"), None, Some(ENTER)));
        s.backend.set_triggers(None, None, Some("Ctrl+Alt+Enter"));
        assert_eq!(
            s.manager.states(),
            states(HotkeyState::Invalid, HotkeyState::Unbound, HotkeyState::Ok)
        );
    }

    #[test]
    fn suspending_does_nothing_where_the_backend_cannot_be_suspended() {
        let s = setup();
        s.backend
            .keeps_hotkeys_on
            .store(true, std::sync::atomic::Ordering::SeqCst);
        s.manager.apply(&HotkeyBindings::default());
        s.backend.clear_log();
        assert!(!s.manager.set_suspended(true), "nothing changed");
        assert!(s.backend.log().is_empty());
        assert_eq!(s.backend.registered(), [SPACE, BACKSPACE, ENTER]);
        assert!(!s.manager.set_suspended(false));
    }

    #[test]
    fn configure_is_the_backends_answer() {
        let s = setup();
        assert_eq!(s.manager.configure(), Ok(ConfigureOutcome::Unsupported));
        *s.backend.configure_answer.lock().unwrap() = Some(Ok(ConfigureOutcome::Opened));
        assert_eq!(s.manager.configure(), Ok(ConfigureOutcome::Opened));
        *s.backend.configure_answer.lock().unwrap() = Some(Err("the portal went away".into()));
        assert_eq!(s.manager.configure(), Err("the portal went away".to_string()));
    }

    #[test]
    fn no_backend_registers_nothing_and_says_so() {
        let fired = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&fired);
        let manager = HotkeyManager::new(
            Box::new(NoBackend),
            Arc::new(move |action| sink.lock().unwrap().push(action)),
        );
        manager.apply(&HotkeyBindings::default());
        assert_eq!(manager.report().backend, HotkeyBackendKind::None);
        assert_eq!(
            manager.states(),
            states(HotkeyState::Unbound, HotkeyState::Unbound, HotkeyState::Unbound)
        );
        assert_eq!(manager.configure(), Ok(ConfigureOutcome::Unsupported));
        assert!(fired.lock().unwrap().is_empty());
    }

    #[test]
    fn the_default_accelerators_all_parse() {
        // Guards the strings the registration passes on to the OS.
        for action in HOTKEY_ACTIONS {
            assert!(parse_accel(HotkeyBindings::default().accel(action).unwrap()).is_ok());
        }
    }
}
