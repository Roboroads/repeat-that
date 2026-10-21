//! `AppAction`: what the tray menu, the hotkeys and the settings window can ask the app to do. Everything funnels
//! through [`dispatch`].

use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};

use crate::engine;
use crate::settings::HOTKEYS;
use crate::window;

/// Something the user asked for, from the tray menu, a hotkey or the settings window.
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

/// What the playback actions drive. Calls return at once. The audio engine implements this
/// ([`AppEngine`](crate::engine::AppEngine)); tests use a fake.
pub trait EngineControl: Send + Sync {
    fn replay(&self);
    fn stop(&self);
    fn toggle_catch_up(&self);
}

/// The engine as managed app state.
pub type SharedEngine = Arc<dyn EngineControl>;

/// What actions need from the app. The real implementation drives the windows and the tray; tests use a fake.
pub trait ActionHost {
    fn show_settings(&self);
    fn quit(&self);
    /// Flips between paused and not paused as one step, so two toggles at once end up back where they started.
    fn toggle_paused(&self);
    fn engine(&self) -> SharedEngine;
}

impl<R: Runtime> ActionHost for AppHandle<R> {
    fn show_settings(&self) {
        window::show_settings(self);
    }

    fn quit(&self) {
        self.exit(0);
    }

    fn toggle_paused(&self) {
        engine::change_paused(self, |paused| !paused);
    }

    fn engine(&self) -> SharedEngine {
        Arc::clone(&self.state::<SharedEngine>())
    }
}

/// The action a command line asks for: `--replay`, `--stop` or `--catch-up` (the names in the hotkey table). Anything
/// else is ignored, so a flag a later version adds, or one the OS adds, never keeps an action from running. With more
/// than one action the first wins.
pub fn cli_action<S: AsRef<str>>(args: &[S]) -> Option<AppAction> {
    // The first argument is the program.
    args.iter().skip(1).find_map(|arg| {
        let flag = arg.as_ref().strip_prefix("--")?;
        HOTKEYS.iter().find(|def| def.command == flag).map(|def| def.action)
    })
}

/// Runs the action a launch asked for, if it asked for one. The first launch of the app does this once the app is up; a
/// launch of an app that already runs is handed to it (see `window::on_second_launch`), which does the same.
pub fn run_launch_action<S: AsRef<str>>(args: &[S], host: &dyn ActionHost) {
    if let Some(action) = cli_action(args) {
        dispatch(action, host);
    }
}

/// Runs `action`.
pub fn dispatch(action: AppAction, host: &dyn ActionHost) {
    match action {
        AppAction::OpenSettings => host.show_settings(),
        AppAction::Quit => host.quit(),
        AppAction::TogglePause => host.toggle_paused(),
        AppAction::Replay => host.engine().replay(),
        AppAction::Stop => host.engine().stop(),
        AppAction::CatchUp => host.engine().toggle_catch_up(),
    }
}

/// Stand-ins for the engine and the app, for tests of everything that dispatches actions.
#[cfg(test)]
pub mod fake {
    use std::cell::{Cell, RefCell};
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct FakeEngine {
        pub calls: Mutex<Vec<&'static str>>,
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
    pub struct FakeHost {
        pub paused: Cell<bool>,
        pub calls: RefCell<Vec<String>>,
        pub engine: Arc<FakeEngine>,
    }

    impl FakeHost {
        pub fn is_paused(&self) -> bool {
            self.paused.get()
        }
    }

    impl ActionHost for FakeHost {
        fn show_settings(&self) {
            self.calls.borrow_mut().push("show_settings".into());
        }

        fn quit(&self) {
            self.calls.borrow_mut().push("quit".into());
        }

        fn toggle_paused(&self) {
            self.paused.set(!self.paused.get());
            self.calls.borrow_mut().push("toggle_paused".into());
        }

        fn engine(&self) -> SharedEngine {
            self.engine.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeHost;
    use super::*;

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
        assert_eq!(run(AppAction::TogglePause, &host), ["toggle_paused"]);
        assert!(host.is_paused());
        dispatch(AppAction::TogglePause, &host);
        assert_eq!(host.calls.borrow().as_slice(), ["toggle_paused", "toggle_paused"]);
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

    // Command-line actions

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn each_playback_flag_asks_for_its_action() {
        assert_eq!(cli_action(&args(&["repeat-that", "--replay"])), Some(AppAction::Replay));
        assert_eq!(cli_action(&args(&["repeat-that", "--stop"])), Some(AppAction::Stop));
        assert_eq!(
            cli_action(&args(&["repeat-that", "--catch-up"])),
            Some(AppAction::CatchUp)
        );
    }

    #[test]
    fn a_plain_launch_asks_for_nothing() {
        assert_eq!(cli_action(&args(&["repeat-that"])), None);
        assert_eq!(cli_action(&args(&[])), None);
    }

    #[test]
    fn unknown_flags_are_ignored() {
        assert_eq!(cli_action(&args(&["repeat-that", "--frobnicate"])), None);
        assert_eq!(
            cli_action(&args(&["repeat-that", "--frobnicate", "--replay", "--verbose"])),
            Some(AppAction::Replay)
        );
    }

    #[test]
    fn an_action_works_with_the_hidden_flag_in_either_order() {
        assert_eq!(
            cli_action(&args(&["repeat-that", "--hidden", "--stop"])),
            Some(AppAction::Stop)
        );
        assert_eq!(
            cli_action(&args(&["repeat-that", "--catch-up", "--hidden"])),
            Some(AppAction::CatchUp)
        );
        assert_eq!(cli_action(&args(&["repeat-that", "--hidden"])), None);
    }

    #[test]
    fn only_the_exact_flag_counts() {
        for flag in [
            "--replayed",
            "--replay=1",
            "replay",
            "-replay",
            "--Replay",
            "--catchup",
            "--catch_up",
        ] {
            assert_eq!(cli_action(&args(&["repeat-that", flag])), None, "{flag}");
        }
    }

    #[test]
    fn with_two_actions_the_first_wins() {
        assert_eq!(
            cli_action(&args(&["repeat-that", "--stop", "--replay"])),
            Some(AppAction::Stop)
        );
    }

    #[test]
    fn the_program_name_is_never_a_flag() {
        assert_eq!(cli_action(&args(&["--replay"])), None);
    }

    #[test]
    fn a_launch_with_an_action_runs_it_and_nothing_else() {
        for (flag, call) in [
            ("--replay", "replay"),
            ("--stop", "stop"),
            ("--catch-up", "toggle_catch_up"),
        ] {
            let host = FakeHost::default();
            run_launch_action(&args(&["repeat-that", flag]), &host);
            assert_eq!(*host.engine.calls.lock().unwrap(), [call], "{flag}");
            assert!(host.calls.borrow().is_empty(), "{flag} must not touch the windows");
        }
    }

    #[test]
    fn a_launch_without_an_action_runs_nothing() {
        let host = FakeHost::default();
        run_launch_action(&args(&["repeat-that", "--hidden", "--frobnicate"]), &host);
        assert!(host.engine.calls.lock().unwrap().is_empty());
        assert!(host.calls.borrow().is_empty());
    }

    #[test]
    fn every_invocation_counts_as_a_press() {
        // Pressing replay again during a replay steps back, and the catch-up action switches speed: both are the
        // engine's rules, which only hear presses. So each command-line call must reach it, with no held-key logic in
        // between that would swallow a second one.
        let host = FakeHost::default();
        for _ in 0..3 {
            run_launch_action(&args(&["repeat-that", "--replay"]), &host);
        }
        run_launch_action(&args(&["repeat-that", "--catch-up"]), &host);
        assert_eq!(
            *host.engine.calls.lock().unwrap(),
            ["replay", "replay", "replay", "toggle_catch_up"]
        );
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
