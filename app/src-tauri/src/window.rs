//! The settings window: when it opens, and that closing it only hides it.

use tauri::{AppHandle, Manager, Runtime, Window, WindowEvent};

use crate::actions::{ActionHost, cli_action, run_launch_action};
use crate::service::{self, SettingsService};
use crate::session::SharedSession;

/// Label of the one window, as in `tauri.conf.json`.
pub const SETTINGS_LABEL: &str = "settings";
/// Launch argument that keeps the window closed. The autostart entry passes it.
pub const HIDDEN_ARG: &str = "--hidden";

/// Whether the launch arguments ask for the window to stay closed.
fn is_hidden_launch<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter().any(|arg| arg.as_ref() == HIDDEN_ARG)
}

/// Whether the settings window opens at startup: on the first launch, and on every launch while there is no tray to
/// open it from. Never with `--hidden` (the autostart entry, which must not pop a window up at every login) and never
/// when the launch is a command-line action, which asks for something to happen, not for the window.
pub fn should_show_at_startup<S: AsRef<str>>(args: &[S], first_run_done: bool, tray_available: bool) -> bool {
    !is_hidden_launch(args) && cli_action(args).is_none() && (!first_run_done || !tray_available)
}

/// Whether a second launch of the app opens the settings window of the running one: always, except with `--hidden` and
/// for a command-line action. The autostart entry passes `--hidden`, so it must not pop the window up when the app
/// already runs (for instance when the user started it by hand before logging in again), and a key bound to
/// `repeat-that --replay` must not either.
pub fn should_show_on_second_launch<S: AsRef<str>>(args: &[S]) -> bool {
    !is_hidden_launch(args) && cli_action(args).is_none()
}

/// Shows the settings window, restoring and focusing it if it is minimized or behind other windows.
///
/// On macOS the app has the accessory activation policy (no Dock icon), which does not by itself bring it to the front.
/// `set_focus` does: tao orders the window front and calls `activateIgnoringOtherApps` on the application (tao 0.37,
/// `util::set_focus`), and `show` has already run by then, which is what that call checks for.
pub fn show_settings<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(SETTINGS_LABEL) else {
        eprintln!("settings window is missing");
        return;
    };
    // Best effort: a window that is already gone has nothing to show.
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// What a second launch does in the running app: runs the action it asks for, or opens the settings window.
fn second_launch<S: AsRef<str>>(host: &dyn ActionHost, args: &[S]) {
    run_launch_action(args, host);
    if should_show_on_second_launch(args) {
        host.show_settings();
    }
}

/// Handles a second launch, whose `args` the single-instance plugin hands to the running app.
pub fn on_second_launch<R: Runtime>(app: &AppHandle<R>, args: &[String]) {
    second_launch(app, args);
}

/// Opens the settings window on the first launch, and on every launch when there is no tray; keeps it closed otherwise,
/// and always with `--hidden` or an action. Needs the [`SettingsService`] in the managed state.
pub fn apply_startup_visibility<R: Runtime>(app: &AppHandle<R>, args: &[String]) {
    let settings = app.state::<SettingsService>();
    let first_run_done = settings.get().first_run_done;
    if should_show_at_startup(args, first_run_done, tray_available(app)) {
        if !first_run_done {
            // Remembered even if this run ends without the window being closed, see `end_first_run_session`.
            settings.begin_first_run_session();
        }
        show_settings(app);
    }
}

/// Whether a tray host is there to show the tray icon. Where the app doesn't look (tests) it is.
fn tray_available<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<SharedSession>()
        .is_none_or(|session| session.tray_available())
}

/// Whether `event` means the window can no longer be recording a hotkey: it lost focus or is closing. The page asks to
/// suspend the hotkeys before recording and to resume them after, but a page that errors or reloads in between never
/// does, and the global hotkeys would stay off while the window stays open.
fn ends_hotkey_recording(event: &WindowEvent) -> bool {
    matches!(event, WindowEvent::Focused(false) | WindowEvent::CloseRequested { .. })
}

/// Closing the settings window hides it; only Quit in the tray ends the app. Closing it also ends the first run. The
/// hotkeys come back whenever the window can no longer be recording one, see [`ends_hotkey_recording`].
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != SETTINGS_LABEL {
        return;
    }
    if ends_hotkey_recording(event) {
        service::suspend_hotkeys(window.app_handle(), false);
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        if let Some(settings) = window.app_handle().try_state::<SettingsService>() {
            settings.mark_first_run_done();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::fake::FakeHost;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    // With a tray, the window opens on the first launch only.

    #[test]
    fn shows_on_the_first_launch() {
        assert!(should_show_at_startup(&args(&["repeat-that"]), false, true));
    }

    #[test]
    fn stays_in_the_tray_after_the_first_launch() {
        assert!(!should_show_at_startup(&args(&["repeat-that"]), true, true));
    }

    #[test]
    fn hidden_argument_wins_over_the_first_launch() {
        assert!(!should_show_at_startup(
            &args(&["repeat-that", "--hidden"]),
            false,
            true
        ));
        assert!(!should_show_at_startup(&args(&["repeat-that", "--hidden"]), true, true));
    }

    #[test]
    fn only_the_exact_hidden_argument_counts() {
        assert!(should_show_at_startup(
            &args(&["repeat-that", "--hiddenly", "hidden"]),
            false,
            true
        ));
    }

    // Without a tray there is nothing to open the window from, so it opens at every launch.

    #[test]
    fn without_a_tray_the_window_opens_at_every_launch() {
        assert!(should_show_at_startup(&args(&["repeat-that"]), true, false));
        assert!(should_show_at_startup(&args(&["repeat-that"]), false, false));
    }

    #[test]
    fn without_a_tray_the_hidden_argument_still_keeps_the_window_closed() {
        // The autostart entry passes it; a login must not pop a window up on a desktop without a tray either.
        assert!(!should_show_at_startup(
            &args(&["repeat-that", "--hidden"]),
            true,
            false
        ));
        assert!(!should_show_at_startup(
            &args(&["repeat-that", "--hidden"]),
            false,
            false
        ));
    }

    #[test]
    fn an_action_launch_never_opens_the_window_at_startup() {
        for flag in ["--replay", "--stop", "--catch-up"] {
            for (first_run_done, tray) in [(false, true), (true, true), (false, false), (true, false)] {
                assert!(
                    !should_show_at_startup(&args(&["repeat-that", flag]), first_run_done, tray),
                    "{flag} first_run_done={first_run_done} tray={tray}"
                );
            }
        }
    }

    #[test]
    fn losing_focus_ends_hotkey_recording() {
        // The recorder lives in the focused window: when focus leaves, the global hotkeys must come back even if the
        // page never says so (an error while recording, a reload).
        assert!(ends_hotkey_recording(&WindowEvent::Focused(false)));
    }

    #[test]
    fn gaining_focus_or_moving_does_not() {
        assert!(!ends_hotkey_recording(&WindowEvent::Focused(true)));
        assert!(!ends_hotkey_recording(&WindowEvent::Moved(
            tauri::PhysicalPosition::new(0, 0)
        )));
    }

    #[test]
    fn a_second_launch_opens_the_window() {
        assert!(should_show_on_second_launch(&args(&["repeat-that"])));
    }

    #[test]
    fn a_second_hidden_launch_leaves_the_window_alone() {
        assert!(!should_show_on_second_launch(&args(&["repeat-that", "--hidden"])));
    }

    #[test]
    fn only_the_exact_hidden_argument_counts_on_a_second_launch() {
        assert!(should_show_on_second_launch(&args(&["repeat-that", "--hiddenly"])));
    }

    #[test]
    fn a_second_launch_with_an_action_does_not_open_the_window() {
        for flag in ["--replay", "--stop", "--catch-up"] {
            assert!(!should_show_on_second_launch(&args(&["repeat-that", flag])), "{flag}");
        }
    }

    fn second(list: &[&str]) -> FakeHost {
        let host = FakeHost::default();
        second_launch(&host, &args(list));
        host
    }

    #[test]
    fn a_second_launch_with_an_action_runs_it_in_the_running_app_and_shows_nothing() {
        let host = second(&["repeat-that", "--replay"]);
        assert_eq!(*host.engine.calls.lock().unwrap(), ["replay"]);
        assert!(host.calls.borrow().is_empty(), "no window: {:?}", host.calls.borrow());

        let host = second(&["repeat-that", "--stop"]);
        assert_eq!(*host.engine.calls.lock().unwrap(), ["stop"]);
        assert!(host.calls.borrow().is_empty());

        let host = second(&["repeat-that", "--catch-up"]);
        assert_eq!(*host.engine.calls.lock().unwrap(), ["toggle_catch_up"]);
        assert!(host.calls.borrow().is_empty());
    }

    #[test]
    fn a_hidden_action_launch_runs_the_action_and_shows_nothing() {
        let host = second(&["repeat-that", "--hidden", "--replay"]);
        assert_eq!(*host.engine.calls.lock().unwrap(), ["replay"]);
        assert!(host.calls.borrow().is_empty());
    }

    #[test]
    fn a_plain_second_launch_shows_the_window_and_runs_nothing() {
        let host = second(&["repeat-that"]);
        assert_eq!(*host.calls.borrow(), ["show_settings"]);
        assert!(host.engine.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_second_launch_with_an_unknown_flag_is_a_plain_one() {
        let host = second(&["repeat-that", "--frobnicate"]);
        assert_eq!(*host.calls.borrow(), ["show_settings"]);
        assert!(host.engine.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_hidden_second_launch_does_nothing() {
        let host = second(&["repeat-that", "--hidden"]);
        assert!(host.calls.borrow().is_empty());
        assert!(host.engine.calls.lock().unwrap().is_empty());
    }
}
