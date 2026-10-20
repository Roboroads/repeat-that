//! Global hotkeys: which combination is registered for which action, whether the OS accepted it, and the rule that a
//! held key counts once.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tauri::{AppHandle, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::actions::AppAction;
use crate::ipc::{HotkeyBindings, HotkeyState, HotkeyStates};
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

/// Registers key combinations with the OS. The real one is the global-shortcut plugin; tests use a fake.
pub trait ShortcutBackend: Send + Sync {
    /// Registers `accel` and calls `on_edge` for every press and release. An error means the OS refused, usually
    /// because another app holds the combination.
    fn register(&self, accel: &str, on_edge: EdgeHandler) -> Result<(), String>;
    /// Best effort: a combination that is already gone has nothing to unregister.
    fn unregister(&self, accel: &str);
}

/// [`ShortcutBackend`] on `tauri-plugin-global-shortcut`.
pub struct PluginBackend<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> ShortcutBackend for PluginBackend<R> {
    fn register(&self, accel: &str, on_edge: EdgeHandler) -> Result<(), String> {
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

    fn unregister(&self, accel: &str) {
        // Best effort, as documented on the trait.
        let _ = self.0.global_shortcut().unregister(accel);
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

    /// Forgets every held key, for when the registrations change under it.
    pub fn reset(&mut self) {
        self.held = [false; HOTKEY_ACTIONS.len()];
    }
}

impl HotkeyStates {
    const UNBOUND: HotkeyStates = HotkeyStates {
        replay: HotkeyState::Unbound,
        stop: HotkeyState::Unbound,
        catch_up: HotkeyState::Unbound,
    };

    fn set(&mut self, action: AppAction, state: HotkeyState) {
        match action {
            AppAction::Replay => self.replay = state,
            AppAction::Stop => self.stop = state,
            AppAction::CatchUp => self.catch_up = state,
            AppAction::TogglePause | AppAction::OpenSettings | AppAction::Quit => {}
        }
    }
}

/// Locks, ignoring poisoning: a panic in one hotkey handler must not take every other hotkey down with it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Inner {
    desired: HotkeyBindings,
    /// Accelerators currently registered with the backend.
    registered: Vec<String>,
    states: HotkeyStates,
    suspended: bool,
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
                registered: Vec::new(),
                states: HotkeyStates::UNBOUND,
                suspended: false,
            }),
        }
    }

    /// Makes `bindings` the registered hotkeys, replacing the old ones. While suspended it only remembers them.
    ///
    /// Registering hops to the main thread and waits for it, so don't call this from a thread the main thread waits on.
    pub fn apply(&self, bindings: &HotkeyBindings) {
        let mut inner = lock(&self.inner);
        inner.desired = bindings.clone();
        self.register_desired(&mut inner);
    }

    /// Unregisters every hotkey (`true`, while the window records a combination) or registers the wanted ones again
    /// (`false`, which also retries the ones another app held).
    pub fn set_suspended(&self, suspended: bool) {
        let mut inner = lock(&self.inner);
        if inner.suspended == suspended {
            return;
        }
        inner.suspended = suspended;
        self.register_desired(&mut inner);
    }

    /// Where each hotkey stands.
    // `Status.hotkeys` reads this once the audio engine fills in the rest of the status; tested until then.
    #[allow(dead_code)]
    pub fn states(&self) -> HotkeyStates {
        lock(&self.inner).states
    }

    /// Drops every registration and registers `inner.desired` again, unless suspended.
    fn register_desired(&self, inner: &mut Inner) {
        for accel in inner.registered.drain(..) {
            self.backend.unregister(&accel);
        }
        lock(&self.tracker).reset();

        let mut states = HotkeyStates::UNBOUND;
        for action in HOTKEY_ACTIONS {
            let state = match inner.desired.accel(action) {
                None => HotkeyState::Unbound,
                Some(accel) if parse_accel(accel).is_err() => HotkeyState::Invalid,
                // While suspended nothing is registered; the window is recording and the keys are free.
                Some(_) if inner.suspended => HotkeyState::Ok,
                Some(accel) => {
                    let accel = accel.trim().to_string();
                    match self.backend.register(&accel, self.edge_handler(action)) {
                        Ok(()) => {
                            inner.registered.push(accel);
                            HotkeyState::Ok
                        }
                        Err(err) => {
                            eprintln!("could not register the {action:?} hotkey {accel}: {err}");
                            HotkeyState::InUse
                        }
                    }
                }
            };
            states.set(action, state);
        }
        inner.states = states;
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

    use super::*;

    #[derive(Default)]
    pub struct FakeBackend {
        pub log: Mutex<Vec<String>>,
        pub handlers: Mutex<Vec<(String, EdgeHandler)>>,
        pub refused: Mutex<HashSet<String>>,
    }

    impl FakeBackend {
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
        fn register(&self, accel: &str, on_edge: EdgeHandler) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("register {accel}"));
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

        fn unregister(&self, accel: &str) {
            self.log.lock().unwrap().push(format!("unregister {accel}"));
            self.handlers.lock().unwrap().retain(|(a, _)| a != accel);
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
    fn reset_forgets_held_keys() {
        let mut tracker = PressTracker::default();
        tracker.on_edge(AppAction::Replay, KeyEdge::Pressed);
        tracker.reset();
        assert!(tracker.on_edge(AppAction::Replay, KeyEdge::Pressed));
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
        assert_eq!(s.backend.registered(), ["Ctrl+Shift+R", BACKSPACE]);
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

    #[test]
    fn the_default_accelerators_all_parse() {
        // Guards the strings the registration passes on to the OS.
        for action in HOTKEY_ACTIONS {
            assert!(parse_accel(HotkeyBindings::default().accel(action).unwrap()).is_ok());
        }
    }
}
