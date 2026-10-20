//! `AppAction`: what the tray menu, the hotkeys and the settings window can ask the app to do. Everything funnels
//! through [`dispatch`].

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

/// What actions need from the app. The real implementation drives the windows and the tray; tests use a fake.
pub trait ActionHost {
    fn show_settings(&self);
    fn quit(&self);
    fn is_paused(&self) -> bool;
    fn set_paused(&self, paused: bool);
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
}

/// Runs `action`. Replay, stop and catch-up have no engine behind them yet and only log.
pub fn dispatch(action: AppAction, host: &dyn ActionHost) {
    match action {
        AppAction::OpenSettings => host.show_settings(),
        AppAction::Quit => host.quit(),
        AppAction::TogglePause => host.set_paused(!host.is_paused()),
        AppAction::Replay | AppAction::Stop | AppAction::CatchUp => {
            eprintln!("{action:?}: no audio engine connected yet");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct FakeHost {
        paused: Cell<bool>,
        calls: RefCell<Vec<String>>,
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
    fn playback_actions_do_nothing_until_the_engine_is_connected() {
        for action in [AppAction::Replay, AppAction::Stop, AppAction::CatchUp] {
            assert!(run(action, &FakeHost::default()).is_empty(), "{action:?}");
        }
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
