//! Repeat that?! desktop shell: tray icon, hidden settings window and the plugins the rest of the app builds on.

mod actions;
#[cfg(test)]
mod mock_runtime_tests;
mod tray;
mod window;

use std::sync::Mutex;

use tauri::{Manager, RunEvent};
use tauri_plugin_autostart::MacosLauncher;

use crate::tray::{SharedStatus, TrayStatus};

/// Starts the app. `rt_capture::duck_guard::run_if_requested()` has already run in `main`, so this is never a duck
/// guard process.
pub fn run() {
    let app = tauri::Builder::default()
        // Start order: single instance first, so a second launch exits before it sets anything up, then the plugins the
        // rest of the app uses, then the tray (in `setup`). The audio engine follows once it exists.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            window::on_second_launch(app, &args)
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::new().build())
        // Starting with the OS is off until the user turns it on; the entry starts the app hidden in the tray.
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![window::HIDDEN_ARG]),
        ))
        .manage::<SharedStatus>(Mutex::new(TrayStatus::default()))
        .on_window_event(window::on_window_event)
        .setup(|app| {
            // A tray app: no Dock icon and no app menu bar on macOS. The tray menu and `RunEvent::Reopen` (the app
            // opened again from Finder or Spotlight) are the ways in.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle();
            tray::setup(handle)?;
            let args: Vec<String> = std::env::args().collect();
            window::apply_startup_visibility(handle, &args);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Repeat that?! app");

    app.run(|app, event| match event {
        RunEvent::ExitRequested { code, api, .. }
            if keep_running_after_exit_request(code, app.webview_windows().len()) =>
        {
            api.prevent_exit();
        }
        // Opening the app again while it runs (Finder, Spotlight, `open`) shows the settings, as a second launch does
        // on the other systems.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => window::show_settings(app),
        _ => {}
    });
}

/// Whether to veto an exit request. Only the loss of the last window is vetoed, so the tray keeps the app alive.
/// Everything else goes through: Quit in the tray asks with an exit code, and while the settings window still exists
/// (it only ever hides) a request without a code is not that case, so an OS-driven quit is never blocked. Tauri has
/// already dropped a closed window from its list when it asks about the exit that follows.
fn keep_running_after_exit_request(code: Option<i32>, windows_left: usize) -> bool {
    code.is_none() && windows_left == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn losing_the_last_window_does_not_end_the_app() {
        assert!(keep_running_after_exit_request(None, 0));
    }

    #[test]
    fn quit_from_the_tray_gets_through() {
        assert!(!keep_running_after_exit_request(Some(0), 0));
        assert!(!keep_running_after_exit_request(Some(0), 1));
    }

    #[test]
    fn a_request_while_the_window_exists_gets_through() {
        // The OS asking the app to quit (Cmd+Q, Dock Quit, logoff) must not be vetoed.
        assert!(!keep_running_after_exit_request(None, 1));
    }
}
