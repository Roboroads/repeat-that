//! `AppAction`: what the tray menu, the hotkeys and the settings window can ask the app to do. Everything funnels
//! through [`dispatch`].

use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};

use crate::tray::{self, SharedStatus};
use crate::window;

/// Something the user asked for, from the tray menu now and from hotkeys and the settings window later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppAction {
    Replay,
    Stop,
    CatchUp,
    TogglePause,
    OpenSettings,
    Quit,
}

impl AppAction {
    pub const ALL: [AppAction; 6] = [
        AppAction::Replay,
        AppAction::Stop,
        AppAction::CatchUp,
        AppAction::TogglePause,
        AppAction::OpenSettings,
        AppAction::Quit,
    ];

    /// Stable id, used for tray menu items.
    pub fn id(self) -> &'static str {
        match self {
            AppAction::Replay => "replay",
            AppAction::Stop => "stop",
            AppAction::CatchUp => "catch_up",
            AppAction::TogglePause => "toggle_pause",
            AppAction::OpenSettings => "open_settings",
            AppAction::Quit => "quit",
        }
    }

    pub fn from_id(id: &str) -> Option<AppAction> {
        AppAction::ALL.into_iter().find(|action| action.id() == id)
    }
}

/// What the playback actions drive. Calls return at once; the audio engine implements this, and until it is connected
/// [`NoEngine`] stands in.
pub trait EngineControl: Send + Sync {
    fn replay(&self);
    fn stop(&self);
    fn toggle_catch_up(&self);
}

/// The engine as managed app state.
pub type SharedEngine = Arc<dyn EngineControl>;

/// Stands in for the audio engine: logs and does nothing.
pub struct NoEngine;

impl EngineControl for NoEngine {
    fn replay(&self) {
        eprintln!("Replay: no audio engine connected yet");
    }

    fn stop(&self) {
        eprintln!("Stop: no audio engine connected yet");
    }

    fn toggle_catch_up(&self) {
        eprintln!("CatchUp: no audio engine connected yet");
    }
}

/// What actions need from the app. The real implementation drives the windows and the tray; tests use a fake.
pub trait ActionHost {
    fn show_settings(&self);
    fn quit(&self);
    fn is_paused(&self) -> bool;
    fn set_paused(&self, paused: bool);
    fn engine(&self) -> SharedEngine;
}

impl<R: Runtime> ActionHost for AppHandle<R> {
    fn show_settings(&self) {
        window::show_settings(self);
    }

    fn quit(&self) {
        self.exit(0);
    }

    fn is_paused(&self) -> bool {
        tray::lock_status(&self.state::<SharedStatus>()).is_paused()
    }

    fn set_paused(&self, paused: bool) {
        tray::lock_status(&self.state::<SharedStatus>()).set_paused(paused);
        tray::refresh(self);
    }

    fn engine(&self) -> SharedEngine {
        Arc::clone(&self.state::<SharedEngine>())
    }
}

/// Runs `action`.
pub fn dispatch(action: AppAction, host: &dyn ActionHost) {
    match action {
        AppAction::OpenSettings => host.show_settings(),
        AppAction::Quit => host.quit(),
        AppAction::TogglePause => host.set_paused(!host.is_paused()),
        AppAction::Replay => host.engine().replay(),
        AppAction::Stop => host.engine().stop(),
        AppAction::CatchUp => host.engine().toggle_catch_up(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeEngine {
        calls: Mutex<Vec<&'static str>>,
    }

    impl EngineControl for FakeEngine {
        fn replay(&self) {
            self.calls.lock().unwrap().push("replay");
        }

        fn stop(&self) {
            self.calls.lock().unwrap().push("stop");
        }

        fn toggle_catch_up(&self) {
            self.calls.lock().unwrap().push("toggle_catch_up");
        }
    }

    #[derive(Default)]
    struct FakeHost {
        paused: Cell<bool>,
        calls: RefCell<Vec<String>>,
        engine: Arc<FakeEngine>,
    }

    impl ActionHost for FakeHost {
        fn show_settings(&self) {
            self.calls.borrow_mut().push("show_settings".into());
        }

        fn quit(&self) {
            self.calls.borrow_mut().push("quit".into());
        }

        fn is_paused(&self) -> bool {
            self.paused.get()
        }

        fn set_paused(&self, paused: bool) {
            self.paused.set(paused);
            self.calls.borrow_mut().push(format!("set_paused({paused})"));
        }

        fn engine(&self) -> SharedEngine {
            self.engine.clone()
        }
    }

    fn run(action: AppAction, host: &FakeHost) -> Vec<String> {
        dispatch(action, host);
        host.calls.borrow().clone()
    }

    #[test]
    fn quit_quits() {
        assert_eq!(run(AppAction::Quit, &FakeHost::default()), ["quit"]);
    }

    #[test]
    fn open_settings_shows_the_window() {
        assert_eq!(run(AppAction::OpenSettings, &FakeHost::default()), ["show_settings"]);
    }

    #[test]
    fn toggle_pause_flips_the_state_both_ways() {
        let host = FakeHost::default();
        assert_eq!(run(AppAction::TogglePause, &host), ["set_paused(true)"]);
        assert!(host.is_paused());
        dispatch(AppAction::TogglePause, &host);
        assert_eq!(
            host.calls.borrow().as_slice(),
            ["set_paused(true)", "set_paused(false)"]
        );
        assert!(!host.is_paused());
    }

    #[test]
    fn playback_actions_reach_the_engine_and_nothing_else() {
        let host = FakeHost::default();
        for action in [AppAction::Replay, AppAction::Stop, AppAction::CatchUp] {
            assert!(run(action, &host).is_empty(), "{action:?} must not touch the windows");
        }
        assert_eq!(
            *host.engine.calls.lock().unwrap(),
            ["replay", "stop", "toggle_catch_up"]
        );
    }

    #[test]
    fn window_actions_leave_the_engine_alone() {
        let host = FakeHost::default();
        for action in [AppAction::OpenSettings, AppAction::TogglePause, AppAction::Quit] {
            dispatch(action, &host);
        }
        assert!(host.engine.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn the_placeholder_engine_ignores_every_call() {
        let engine = NoEngine;
        engine.replay();
        engine.stop();
        engine.toggle_catch_up();
    }

    #[test]
    fn ids_round_trip_and_are_unique() {
        for action in AppAction::ALL {
            assert_eq!(AppAction::from_id(action.id()), Some(action));
        }
        let mut ids: Vec<_> = AppAction::ALL.iter().map(|a| a.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), AppAction::ALL.len());
        assert_eq!(AppAction::from_id("status"), None);
        assert_eq!(AppAction::from_id(""), None);
    }
}
