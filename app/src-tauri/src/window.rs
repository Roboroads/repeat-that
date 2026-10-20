//! The settings window: when it opens, and that closing it only hides it.

use serde_json::{Map, Value};
use tauri::{AppHandle, Manager, Runtime, Window, WindowEvent};
use tauri_plugin_store::StoreExt;

/// Label of the one window, as in `tauri.conf.json`.
pub const SETTINGS_LABEL: &str = "settings";
/// Launch argument that keeps the window closed. The autostart entry passes it.
pub const HIDDEN_ARG: &str = "--hidden";

/// Key of the settings object in `settings.json`; the settings layer owns everything else in it.
const SETTINGS_KEY: &str = "settings";
const FIRST_RUN_KEY: &str = "firstRunDone";
const STORE_FILE: &str = "settings.json";

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

/// `firstRunDone` in the stored settings object; false when missing or not a boolean.
pub fn first_run_done(settings: Option<&Value>) -> bool {
    settings
        .and_then(|s| s.get(FIRST_RUN_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The stored settings object with `firstRunDone` set, other fields kept. Anything that is not an object is replaced.
pub fn with_first_run_done(settings: Option<Value>) -> Value {
    let mut object = match settings {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    object.insert(FIRST_RUN_KEY.into(), Value::Bool(true));
    Value::Object(object)
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

/// Handles a second launch, whose `args` the single-instance plugin hands to the running app.
pub fn on_second_launch<R: Runtime>(app: &AppHandle<R>, args: &[String]) {
    if should_show_on_second_launch(args) {
        show_settings(app);
    }
}

/// Opens the settings window on the first launch and keeps it closed otherwise, and always with `--hidden`.
pub fn apply_startup_visibility<R: Runtime>(app: &AppHandle<R>, args: &[String]) {
    if should_show_at_startup(args, read_first_run_done(app)) {
        show_settings(app);
    }
}

/// Closing the settings window hides it; only Quit in the tray ends the app. Closing it also ends the first run.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != SETTINGS_LABEL {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        mark_first_run_done(window.app_handle());
    }
}

/// `settings.json` in the app config dir.
fn settings_file<R: Runtime>(app: &AppHandle<R>) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join(STORE_FILE))
}

/// The stored first-run flag. A missing or unreadable settings file counts as a first run.
fn read_first_run_done<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(path) = settings_file(app) else {
        return false;
    };
    match app.store(path) {
        Ok(store) => first_run_done(store.get(SETTINGS_KEY).as_ref()),
        Err(err) => {
            eprintln!("could not read the settings file: {err}");
            false
        }
    }
}

fn mark_first_run_done<R: Runtime>(app: &AppHandle<R>) {
    let Some(path) = settings_file(app) else {
        return;
    };
    match app.store(path) {
        Ok(store) => {
            let current = store.get(SETTINGS_KEY);
            if first_run_done(current.as_ref()) {
                return;
            }
            store.set(SETTINGS_KEY, with_first_run_done(current));
            if let Err(err) = store.save() {
                eprintln!("could not save the settings file: {err}");
            }
        }
        Err(err) => eprintln!("could not open the settings file: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    #[test]
    fn first_run_flag_is_read_from_the_settings_object() {
        assert!(!first_run_done(None));
        assert!(!first_run_done(Some(&json!({}))));
        assert!(!first_run_done(Some(&json!({ "firstRunDone": false }))));
        assert!(first_run_done(Some(
            &json!({ "firstRunDone": true, "bufferSeconds": 30 })
        )));
    }

    #[test]
    fn a_broken_settings_value_counts_as_first_run() {
        assert!(!first_run_done(Some(&json!({ "firstRunDone": "yes" }))));
        assert!(!first_run_done(Some(&json!("garbage"))));
        assert!(!first_run_done(Some(&json!(null))));
    }

    #[test]
    fn marking_the_first_run_done_keeps_other_settings() {
        let marked = with_first_run_done(Some(json!({ "bufferSeconds": 45, "firstRunDone": false })));
        assert_eq!(marked, json!({ "bufferSeconds": 45, "firstRunDone": true }));
    }

    #[test]
    fn marking_the_first_run_done_without_settings_creates_them() {
        assert_eq!(with_first_run_done(None), json!({ "firstRunDone": true }));
        assert_eq!(
            with_first_run_done(Some(json!([1, 2]))),
            json!({ "firstRunDone": true })
        );
    }
}
