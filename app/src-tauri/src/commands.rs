//! The commands of the contract with the settings window (`app/src/lib/api.ts`). They are thin: the rules live in
//! [`SettingsService`](crate::service::SettingsService) and the audio engine.
//!
//! Most are plain (not `async`) commands on purpose: those run on the main thread, which is where registering hotkeys
//! happens anyway, so an update never waits on the main thread from another thread. The two that ask the OS for its
//! apps or devices can take a while, so they are `async` and do that on a thread of their own.

use std::sync::Arc;

use rt_capture::CaptureError;
use rt_engine::EngineHandle;
use serde_json::{Value, json};
use tauri::ipc::Invoke;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::actions::{self, AppAction};
use crate::engine::{self, SharedAppEngine};
use crate::icons::SharedIcons;
use crate::ipc::{AppEntry, CommandError, ConfigureOutcome, OutputDevice, Settings, Status};
use crate::service::{self, SettingsService};

/// Every command, for the app and its tests alike.
pub fn handler<R: Runtime>() -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        get_settings,
        update_settings,
        suspend_hotkeys,
        get_status,
        list_apps,
        list_output_devices,
        select_app,
        set_paused,
        replay,
        stop,
        catch_up,
        configure_shortcuts,
        quit_app,
    ]
}

#[tauri::command]
pub fn get_settings(settings: State<'_, SettingsService>) -> Settings {
    settings.get()
}

/// `patch` is a partial `Settings`; see [`SettingsPatch`](crate::ipc::SettingsPatch). It arrives as plain JSON so a
/// malformed one is answered with a [`CommandError`] like every other failure.
#[tauri::command]
pub fn update_settings(settings: State<'_, SettingsService>, patch: Value) -> Result<Settings, CommandError> {
    settings.update(patch)
}

/// Unregisters the hotkeys while the window records a combination (`true`) and registers them again afterwards.
#[tauri::command]
pub fn suspend_hotkeys<R: Runtime>(app: AppHandle<R>, suspended: bool) {
    service::suspend_hotkeys(&app, suspended);
}

#[tauri::command]
pub fn get_status<R: Runtime>(app: AppHandle<R>) -> Status {
    engine::current_status(&app)
}

/// The apps playing audio, with their icons.
#[tauri::command]
pub async fn list_apps<R: Runtime>(app: AppHandle<R>) -> Result<Vec<AppEntry>, CommandError> {
    let engine = running_engine(&app)?;
    let icons = Arc::clone(&app.state::<SharedIcons>());
    off_the_main_thread(move || {
        let apps = engine.list_apps().map_err(capture_error)?;
        Ok(apps.into_iter().map(|app| icons.entry(app)).collect())
    })
    .await
}

#[tauri::command]
pub async fn list_output_devices<R: Runtime>(app: AppHandle<R>) -> Result<Vec<OutputDevice>, CommandError> {
    let engine = running_engine(&app)?;
    off_the_main_thread(move || {
        let devices = engine
            .list_output_devices()
            .map_err(|err| CommandError::backend(format!("Could not list the output devices: {err}")))?;
        Ok(devices
            .into_iter()
            .map(|device| OutputDevice {
                id: device.id,
                name: device.name,
                is_default: device.is_default,
            })
            .collect())
    })
    .await
}

/// Makes `app` the target: it is captured from now on (once it plays audio), and what was buffered of another app is
/// wiped. Also ends the first run. The window sends it without its icon.
#[tauri::command]
pub fn select_app(settings: State<'_, SettingsService>, app: AppEntry) -> Result<(), CommandError> {
    settings
        .update(json!({ "target": { "exePath": app.exe_path, "name": app.name } }))
        .map(drop)
}

/// Pausing stops capture and wipes the buffer; resuming captures again.
#[tauri::command]
pub fn set_paused<R: Runtime>(app: AppHandle<R>, paused: bool) {
    engine::change_paused(&app, |_| paused);
}

#[tauri::command]
pub fn replay<R: Runtime>(app: AppHandle<R>) {
    actions::dispatch(AppAction::Replay, &app);
}

#[tauri::command]
pub fn stop<R: Runtime>(app: AppHandle<R>) {
    actions::dispatch(AppAction::Stop, &app);
}

#[tauri::command]
pub fn catch_up<R: Runtime>(app: AppHandle<R>) {
    actions::dispatch(AppAction::CatchUp, &app);
}

/// Asks the desktop to show where the user changes the hotkeys (Wayland, where the desktop owns the keys). Answers
/// `unsupported` where it can't be asked, and the window then says where in the system settings to look.
#[tauri::command]
pub async fn configure_shortcuts<R: Runtime>(app: AppHandle<R>) -> Result<ConfigureOutcome, CommandError> {
    // The desktop answers over D-Bus, which can take a moment: not on the main thread.
    off_the_main_thread(move || app.state::<SettingsService>().configure_shortcuts()).await
}

/// Quits the app, for a desktop with no tray, where the tray menu's Quit isn't there.
#[tauri::command]
pub fn quit_app<R: Runtime>(app: AppHandle<R>) {
    actions::dispatch(AppAction::Quit, &app);
}

fn running_engine<R: Runtime>(app: &AppHandle<R>) -> Result<EngineHandle, CommandError> {
    app.try_state::<SharedAppEngine>()
        .and_then(|engine| engine.handle())
        .ok_or_else(|| CommandError::backend("The audio engine isn't running"))
}

fn capture_error(err: CaptureError) -> CommandError {
    match err {
        // Already a sentence for the user.
        CaptureError::Unsupported(why) => CommandError::backend(why),
        other => CommandError::backend(format!("Could not list the apps: {other}")),
    }
}

/// Runs `work` on a thread for blocking work and waits for it without holding up a command thread.
async fn off_the_main_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .unwrap_or_else(|err| Err(CommandError::backend(format!("The request failed: {err}"))))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use tauri::Listener;

    use super::*;
    use crate::engine::STATUS_EVENT;
    use crate::service::SETTINGS_CHANGED_EVENT;
    use crate::test_app::TestApp;
    use crate::tray::{self, CaptureView, SharedStatus, TrayState};

    fn harness() -> TestApp {
        TestApp::without_engine()
    }

    fn invoke(h: &TestApp, cmd: &str, args: serde_json::Value) -> Result<serde_json::Value, serde_json::Value> {
        h.invoke(cmd, args)
    }

    /// The commands `api.ts` calls, as `call<…>('name'`.
    fn commands_the_window_calls() -> Vec<String> {
        let api = std::fs::read_to_string("../src/lib/api.ts").expect("the window's API");
        api.split("call")
            .skip(1)
            .filter_map(|rest| {
                let rest = rest
                    .strip_prefix('<')
                    .map_or(rest, |typed| typed.split_once('>').map_or("", |(_, r)| r));
                let name = rest.strip_prefix("('")?.split_once('\'')?.0;
                Some(name.to_string())
            })
            .collect()
    }

    #[test]
    fn every_command_the_window_calls_is_there() {
        let h = harness();
        let unknown = invoke(&h, "no_such_command", json!({})).unwrap_err();
        assert!(unknown.to_string().contains("not found"), "{unknown}");
        let commands = commands_the_window_calls();
        assert!(commands.len() >= 11, "{commands:?}");
        for cmd in commands {
            if cmd == "quit_app" {
                // Ends the app, which the mock runtime can't do; see `the_quit_command_is_registered`.
                continue;
            }
            // Answered or refused for a reason of its own, never refused as unknown.
            if let Err(error) = invoke(&h, &cmd, json!({ "patch": {}, "paused": false, "suspended": false })) {
                assert!(!error.to_string().contains("not found"), "{cmd}: {error}");
            }
        }
    }

    #[test]
    fn get_settings_returns_the_defaults_in_the_contract_shape() {
        let h = harness();
        let settings = invoke(&h, "get_settings", json!({})).unwrap();
        assert_eq!(settings, serde_json::to_value(Settings::default()).unwrap());
        assert_eq!(settings["bufferSeconds"], 30);
        assert_eq!(settings["hotkeys"]["catchUp"], "Ctrl+Alt+Enter");
        assert_eq!(settings["version"], 1);
    }

    #[test]
    fn update_settings_round_trips_with_get_settings() {
        let h = harness();
        let updated = invoke(
            &h,
            "update_settings",
            json!({ "patch": { "bufferSeconds": 60, "sensitivity": "high", "duck": { "enabled": false } } }),
        )
        .unwrap();
        assert_eq!(updated["bufferSeconds"], 60);
        assert_eq!(updated["sensitivity"], "high");
        assert_eq!(updated["duck"], json!({ "enabled": false, "level": 0.2 }));
        assert_eq!(invoke(&h, "get_settings", json!({})).unwrap(), updated);
        assert_eq!(h.persistence.saves().len(), 1, "saved once");
    }

    #[test]
    fn update_settings_announces_the_new_settings() {
        let h = harness();
        assert!(h.events(SETTINGS_CHANGED_EVENT).is_empty());
        let updated = invoke(&h, "update_settings", json!({ "patch": { "bufferSeconds": 45 } })).unwrap();
        assert_eq!(h.events(SETTINGS_CHANGED_EVENT), [updated]);
        // Setting what is already set announces nothing.
        invoke(&h, "update_settings", json!({ "patch": { "bufferSeconds": 45 } })).unwrap();
        assert_eq!(h.events(SETTINGS_CHANGED_EVENT).len(), 1);
    }

    #[test]
    fn update_settings_registers_a_changed_hotkey() {
        let h = harness();
        invoke(
            &h,
            "update_settings",
            json!({ "patch": { "hotkeys": { "replay": "Ctrl+Shift+R" } } }),
        )
        .unwrap();
        assert_eq!(
            h.backend.registered_sorted(),
            ["Ctrl+Alt+Backspace", "Ctrl+Alt+Enter", "Ctrl+Shift+R"]
        );
    }

    #[test]
    fn a_conflict_rejects_with_a_command_error_and_changes_nothing() {
        let h = harness();
        let error = invoke(
            &h,
            "update_settings",
            json!({ "patch": { "bufferSeconds": 90, "hotkeys": { "stop": "Ctrl+Alt+Space" } } }),
        )
        .unwrap_err();
        assert_eq!(error["kind"], "conflict");
        assert!(error["message"].as_str().unwrap().contains("Stop"), "{error}");
        assert_eq!(invoke(&h, "get_settings", json!({})).unwrap()["bufferSeconds"], 30);
        assert!(h.events(SETTINGS_CHANGED_EVENT).is_empty());
        assert!(h.persistence.saves().is_empty());
    }

    #[test]
    fn a_bad_hotkey_rejects_as_invalid() {
        let h = harness();
        let error = invoke(
            &h,
            "update_settings",
            json!({ "patch": { "hotkeys": { "replay": "Ctrl+Alt+Frobnicate" } } }),
        )
        .unwrap_err();
        assert_eq!(error["kind"], "invalid");
        assert!(error["message"].as_str().unwrap().contains("Frobnicate"), "{error}");
    }

    #[test]
    fn a_malformed_patch_rejects_as_invalid_not_as_a_plain_string() {
        let h = harness();
        let error = invoke(&h, "update_settings", json!({ "patch": { "bufferSeconds": "lots" } })).unwrap_err();
        assert_eq!(error["kind"], "invalid");
        assert!(error["message"].is_string());
    }

    #[test]
    fn a_failing_save_rejects_as_backend() {
        let h = harness();
        h.persistence.fail_saves(true);
        let error = invoke(&h, "update_settings", json!({ "patch": { "bufferSeconds": 90 } })).unwrap_err();
        assert_eq!(error["kind"], "backend");
        assert_eq!(invoke(&h, "get_settings", json!({})).unwrap()["bufferSeconds"], 30);
    }

    #[test]
    fn suspend_hotkeys_unregisters_and_registers_again() {
        let h = harness();
        assert_eq!(h.backend.registered().len(), 3);
        invoke(&h, "suspend_hotkeys", json!({ "suspended": true })).unwrap();
        assert!(h.backend.registered().is_empty());
        invoke(&h, "suspend_hotkeys", json!({ "suspended": false })).unwrap();
        assert_eq!(h.backend.registered().len(), 3);
    }

    #[test]
    fn configure_shortcuts_says_so_when_the_desktop_cannot_show_its_settings() {
        let h = harness();
        assert_eq!(invoke(&h, "configure_shortcuts", json!({})).unwrap(), "unsupported");
    }

    #[test]
    fn configure_shortcuts_says_when_the_desktop_opened_its_settings() {
        let h = harness();
        *h.backend.configure_answer.lock().unwrap() = Some(Ok(crate::ipc::ConfigureOutcome::Opened));
        assert_eq!(invoke(&h, "configure_shortcuts", json!({})).unwrap(), "opened");
    }

    #[test]
    fn configure_shortcuts_rejects_as_backend_when_the_desktop_fails() {
        let h = harness();
        *h.backend.configure_answer.lock().unwrap() = Some(Err("the portal went away".into()));
        let error = invoke(&h, "configure_shortcuts", json!({})).unwrap_err();
        assert_eq!(error["kind"], "backend");
        assert!(
            error["message"].as_str().unwrap().contains("the portal went away"),
            "{error}"
        );
    }

    #[test]
    fn the_quit_command_is_registered() {
        // Invoking it would end the process, and the mock runtime can't stand in for that. `every_command_the_window_calls_is_there`
        // finds it in api.ts; this is the other half, that the handler list has it.
        let source = std::fs::read_to_string("src/commands.rs").expect("this file");
        let handler = source
            .split("tauri::generate_handler![")
            .nth(1)
            .expect("the handler list");
        assert!(handler.split(']').next().unwrap().contains("quit_app"));
    }

    // The status, and the commands of the audio engine, on the real engine capturing the fixture.

    /// Plenty for the engine to find the fake app (it looks at once) and its first speech (within a second or two).
    const PATIENCE: Duration = Duration::from_secs(15);

    fn state(status: &Value) -> &str {
        status["capture"]["state"].as_str().expect("a capture state")
    }

    fn buffered_ms(status: &Value) -> u64 {
        status["bufferedMs"].as_u64().expect("bufferedMs")
    }

    fn the_mock_app(h: &TestApp) -> Value {
        let apps = invoke(h, "list_apps", json!({})).unwrap();
        apps[0].clone()
    }

    /// Picks `app` the way the window does: without its icon.
    fn select(h: &TestApp, app: &Value) {
        let mut app = app.clone();
        app["icon"] = Value::Null;
        invoke(h, "select_app", json!({ "app": app })).unwrap();
    }

    fn capturing_with_speech(h: &TestApp) -> Value {
        select(h, &the_mock_app(h));
        h.wait_for_status(PATIENCE, "speech in the buffer", |s| {
            state(s) == "capturing" && buffered_ms(s) > 0
        })
    }

    fn tray(h: &TestApp) -> TrayState {
        tray::lock_status(&h.app.state::<SharedStatus>()).clone()
    }

    #[test]
    fn list_apps_returns_the_mock_app_without_an_icon() {
        let h = TestApp::with_engine();
        assert_eq!(
            invoke(&h, "list_apps", json!({})).unwrap(),
            json!([{ "pid": u32::MAX, "name": "speech_8s", "exePath": null, "icon": null }])
        );
    }

    #[test]
    fn list_output_devices_lists_the_outputs() {
        let h = TestApp::with_engine();
        assert_eq!(
            invoke(&h, "list_output_devices", json!({})).unwrap(),
            json!([{ "id": "speakers", "name": "speakers", "isDefault": true }])
        );
    }

    #[test]
    fn without_the_engine_the_lists_are_a_backend_error() {
        let h = harness();
        for cmd in ["list_apps", "list_output_devices"] {
            let error = invoke(&h, cmd, json!({})).unwrap_err();
            assert_eq!(error["kind"], "backend", "{cmd}");
            assert!(error["message"].as_str().unwrap().contains("engine"), "{cmd}: {error}");
        }
    }

    #[test]
    fn get_status_starts_with_no_target_and_the_hotkeys_registered() {
        let h = TestApp::with_engine();
        let status = invoke(&h, "get_status", json!({})).unwrap();
        assert_eq!(status["capture"], json!({ "state": "noTarget", "message": null }));
        assert_eq!(status["playback"], "idle");
        assert_eq!(status["behindMs"], Value::Null);
        assert_eq!(
            status["hotkeys"],
            json!({ "replay": "ok", "stop": "ok", "catchUp": "ok" })
        );
        assert_eq!(status["settingsProblem"], Value::Null);
    }

    #[test]
    fn select_app_captures_it_and_keeps_it_as_the_target() {
        let h = TestApp::with_engine();
        select(&h, &the_mock_app(&h));
        let status = h.wait_for_status(PATIENCE, "capturing", |s| state(s) == "capturing");
        assert_eq!(status["targetName"], "speech_8s");
        let settings = invoke(&h, "get_settings", json!({})).unwrap();
        assert_eq!(settings["target"], json!({ "exePath": null, "name": "speech_8s" }));
        assert_eq!(settings["firstRunDone"], true, "picking an app ends the first run");
    }

    #[test]
    fn select_app_needs_a_name() {
        let h = TestApp::with_engine();
        let error = invoke(
            &h,
            "select_app",
            json!({ "app": { "pid": 1, "name": " ", "exePath": null, "icon": null } }),
        )
        .unwrap_err();
        assert_eq!(error["kind"], "invalid");
    }

    #[test]
    fn the_stored_target_is_captured_from_the_start() {
        let h = TestApp::with_engine_and_settings(json!({ "target": { "exePath": null, "name": "speech_8s" } }));
        h.wait_for_status(PATIENCE, "capturing", |s| state(s) == "capturing");
    }

    #[test]
    fn the_engine_status_goes_to_the_window_and_the_tray() {
        let h = TestApp::with_engine();
        select(&h, &the_mock_app(&h));
        h.wait_for_event(PATIENCE, "capturing", |s| state(s) == "capturing");
        h.wait_until(PATIENCE, "the tray to show the capture", || {
            tray(&h).status.capture == CaptureView::Capturing("speech_8s".into())
        });
        assert_eq!(tray(&h).hotkeys.replay.as_deref(), Some("Ctrl+Alt+Space"));
    }

    #[test]
    fn replay_and_stop_change_the_playback() {
        let h = TestApp::with_engine();
        capturing_with_speech(&h);
        invoke(&h, "replay", json!({})).unwrap();
        h.wait_for_status(PATIENCE, "replaying", |s| s["playback"] == "replaying");
        invoke(&h, "stop", json!({})).unwrap();
        h.wait_for_status(PATIENCE, "idle again", |s| s["playback"] == "idle");
        assert!(!h.output.is_empty(), "the replay played");
    }

    #[test]
    fn catch_up_replays_at_catch_up_speed() {
        let h = TestApp::with_engine();
        capturing_with_speech(&h);
        invoke(&h, "catch_up", json!({})).unwrap();
        h.wait_for_status(PATIENCE, "catching up", |s| s["playback"] == "catchingUp");
        invoke(&h, "stop", json!({})).unwrap();
    }

    #[test]
    fn set_paused_wipes_the_buffer_and_resuming_captures_again() {
        let h = TestApp::with_engine();
        capturing_with_speech(&h);
        invoke(&h, "set_paused", json!({ "paused": true })).unwrap();
        h.wait_for_status(PATIENCE, "paused with an empty buffer", |s| {
            state(s) == "paused" && buffered_ms(s) == 0
        });
        h.wait_until(PATIENCE, "the tray's Pause checkbox ticked", || {
            tray(&h).status.is_paused()
        });
        invoke(&h, "set_paused", json!({ "paused": false })).unwrap();
        h.wait_for_status(PATIENCE, "capturing again", |s| state(s) == "capturing");
        h.wait_until(PATIENCE, "the tray capturing again", || {
            tray(&h).status.capture == CaptureView::Capturing("speech_8s".into())
        });
    }

    #[test]
    fn the_tray_shows_pausing_as_the_window_does() {
        // Without an app the engine has nothing to pause and the window says so; the tray must not claim otherwise.
        let h = TestApp::with_engine();
        invoke(&h, "set_paused", json!({ "paused": true })).unwrap();
        assert_eq!(state(&invoke(&h, "get_status", json!({})).unwrap()), "noTarget");
        assert!(!tray(&h).status.is_paused());
        invoke(&h, "set_paused", json!({ "paused": false })).unwrap();
        assert!(!tray(&h).status.is_paused());
    }

    #[test]
    fn settings_changes_reach_the_engine() {
        let h = TestApp::with_engine();
        invoke(
            &h,
            "update_settings",
            json!({ "patch": { "bufferSeconds": 60, "outputDevice": "speakers", "duck": { "enabled": false } } }),
        )
        .unwrap();
        let config = h.app.state::<SharedAppEngine>().config().expect("the engine runs");
        assert_eq!(config.buffer, Duration::from_secs(60));
        assert_eq!(config.output_device.as_deref(), Some("speakers"));
        assert_eq!(config.duck, None);
    }

    #[test]
    fn update_settings_sends_the_status_again() {
        let h = harness();
        h.backend.refuse("Ctrl+Shift+R");
        h.clear_events();
        invoke(
            &h,
            "update_settings",
            json!({ "patch": { "hotkeys": { "replay": "Ctrl+Shift+R" } } }),
        )
        .unwrap();
        h.wait_for_event(PATIENCE, "replay in use", |s| s["hotkeys"]["replay"] == "inUse");
        // The tray names no hotkey that doesn't work.
        h.wait_until(PATIENCE, "the tray without the replay hotkey", || {
            tray(&h).hotkeys.replay.is_none()
        });
        assert_eq!(tray(&h).hotkeys.stop.as_deref(), Some("Ctrl+Alt+Backspace"));
    }

    #[test]
    fn hotkeys_coming_back_send_the_status_again() {
        let h = harness();
        invoke(&h, "suspend_hotkeys", json!({ "suspended": true })).unwrap();
        h.backend.refuse("Ctrl+Alt+Space");
        h.clear_events();
        invoke(&h, "suspend_hotkeys", json!({ "suspended": false })).unwrap();
        h.wait_for_event(PATIENCE, "replay in use", |s| s["hotkeys"]["replay"] == "inUse");
    }

    #[test]
    fn a_status_listener_can_change_the_settings() {
        // A listener runs while the status is sent; what it changes is sent again, without waiting on that first send.
        let h = harness();
        let handle = h.app.handle().clone();
        let once = Arc::new(std::sync::atomic::AtomicBool::new(false));
        h.app.listen(STATUS_EVENT, move |_| {
            if !once.swap(true, std::sync::atomic::Ordering::SeqCst) {
                let settings = handle.state::<SettingsService>();
                settings
                    .update(json!({ "bufferSeconds": 45 }))
                    .expect("the update goes through");
            }
        });
        let (done, finished) = std::sync::mpsc::channel();
        let handle = h.app.handle().clone();
        std::thread::spawn(move || {
            engine::publish_status(&handle);
            let _ = done.send(());
        });
        finished
            .recv_timeout(PATIENCE)
            .expect("sending the status is not stuck");
        h.wait_until(PATIENCE, "the listener's change", || {
            h.app.state::<SettingsService>().get().buffer_seconds == 45
        });
        h.wait_for_event(PATIENCE, "the status sent again", |_| {
            h.events(SETTINGS_CHANGED_EVENT).len() == 1
        });
    }

    #[test]
    fn a_settings_problem_shows_in_the_status() {
        let h = harness();
        h.persistence.set_problem(Some("The settings file is read-only"));
        assert_eq!(
            invoke(&h, "get_status", json!({})).unwrap()["settingsProblem"],
            "The settings file is read-only"
        );
    }
}
