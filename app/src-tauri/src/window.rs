//! The settings window: when it opens, and that closing it only hides it.

use tauri::{AppHandle, Manager, Runtime, Window, WindowEvent};

use crate::service::SettingsService;

/// Label of the one window, as in `tauri.conf.json`.
pub const SETTINGS_LABEL: &str = "settings";
/// Launch argument that keeps the window closed. The autostart entry passes it.
pub const HIDDEN_ARG: &str = "--hidden";

/// Whether the launch arguments ask for the window to stay closed.
fn is_hidden_launch<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter().any(|arg| arg.as_ref() == HIDDEN_ARG)
}

/// Whether the settings window opens at startup: on the first launch only, and never with `--hidden`.
pub fn should_show_at_startup<S: AsRef<str>>(args: &[S], first_run_done: bool) -> bool {
    !is_hidden_launch(args) && !first_run_done
}

/// Whether a second launch of the app opens the settings window of the running one: always, except with `--hidden`.
/// The autostart entry passes `--hidden`, so it must not pop the window up when the app already runs (for instance
/// when the user started it by hand before logging in again).
pub fn should_show_on_second_launch<S: AsRef<str>>(args: &[S]) -> bool {
    !is_hidden_launch(args)
}

/// Shows the settings window, restoring and focusing it if it is minimized or behind other windows.
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

/// Handles a second launch, whose `args` the single-instance plugin hands to the running app.
pub fn on_second_launch<R: Runtime>(app: &AppHandle<R>, args: &[String]) {
    if should_show_on_second_launch(args) {
        show_settings(app);
    }
}

/// Opens the settings window on the first launch and keeps it closed otherwise, and always with `--hidden`. Needs the
/// [`SettingsService`] in the managed state.
pub fn apply_startup_visibility<R: Runtime>(app: &AppHandle<R>, args: &[String]) {
    let settings = app.state::<SettingsService>();
    if should_show_at_startup(args, settings.get().first_run_done) {
        // Remembered even if this run ends without the window being closed, see `end_first_run_session`.
        settings.begin_first_run_session();
        show_settings(app);
    }
}

/// Closing the settings window hides it; only Quit in the tray ends the app. Closing it also ends the first run, and
/// gives the hotkeys back if the window was still recording one.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != SETTINGS_LABEL {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        if let Some(settings) = window.app_handle().try_state::<SettingsService>() {
            settings.suspend_hotkeys(false);
            settings.mark_first_run_done();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn shows_on_the_first_launch() {
        assert!(should_show_at_startup(&args(&["repeat-that"]), false));
    }

    #[test]
    fn stays_in_the_tray_after_the_first_launch() {
        assert!(!should_show_at_startup(&args(&["repeat-that"]), true));
    }

    #[test]
    fn hidden_argument_wins_over_the_first_launch() {
        assert!(!should_show_at_startup(&args(&["repeat-that", "--hidden"]), false));
        assert!(!should_show_at_startup(&args(&["repeat-that", "--hidden"]), true));
    }

    #[test]
    fn only_the_exact_hidden_argument_counts() {
        assert!(should_show_at_startup(
            &args(&["repeat-that", "--hiddenly", "hidden"]),
            false
        ));
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
}
