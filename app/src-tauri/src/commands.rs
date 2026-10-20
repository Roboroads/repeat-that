//! The commands of the settings contract (`get_settings`, `update_settings`, `suspend_hotkeys`). They are thin: the
//! rules live in [`SettingsService`](crate::service::SettingsService).
//!
//! They are plain (not `async`) commands on purpose: those run on the main thread, which is where registering hotkeys
//! happens anyway, so an update never waits on the main thread from another thread.

use serde_json::Value;
use tauri::State;

use crate::ipc::{CommandError, Settings};
use crate::service::SettingsService;

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
pub fn suspend_hotkeys(settings: State<'_, SettingsService>, suspended: bool) {
    settings.suspend_hotkeys(suspended);
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;
    use tauri::{Listener, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

    use super::*;
    use crate::autostart::fake::FakeAutostart;
    use crate::hotkeys::fake::FakeBackend;
    use crate::service::fake::MemoryPersistence;
    use crate::service::{SETTINGS_CHANGED_EVENT, install_with};

    /// A mock app with the settings commands, fake storage, autostart and shortcuts, and a window to invoke from.
    struct Harness {
        // Kept alive: dropping the app tears the window down.
        _app: tauri::App<MockRuntime>,
        window: WebviewWindow<MockRuntime>,
        persistence: Arc<MemoryPersistence>,
        backend: Arc<FakeBackend>,
        events: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    fn harness() -> Harness {
        let app = mock_builder()
            .invoke_handler(tauri::generate_handler![get_settings, update_settings, suspend_hotkeys])
            .build(mock_context(noop_assets()))
            .expect("mock app builds");
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        app.listen(SETTINGS_CHANGED_EVENT, move |event| {
            sink.lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).expect("the event carries JSON"));
        });
        let persistence = Arc::new(MemoryPersistence::default());
        let backend = Arc::new(FakeBackend::default());
        install_with(
            app.handle(),
            persistence.clone(),
            Arc::new(FakeAutostart::default()),
            Box::new(Arc::clone(&backend)),
        );
        let window = WebviewWindowBuilder::new(&app, "settings", WebviewUrl::default())
            .build()
            .expect("mock window builds");
        Harness {
            _app: app,
            window,
            persistence,
            backend,
            events,
        }
    }

    /// Invokes `cmd` the way the frontend does and returns what the promise would resolve or reject with.
    fn invoke(h: &Harness, cmd: &str, args: serde_json::Value) -> Result<serde_json::Value, serde_json::Value> {
        // The origin of the app's own pages, which is what lets a command through without a capability.
        let origin = if cfg!(windows) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        get_ipc_response(
            &h.window,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: origin.parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize().expect("the response is JSON"))
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
        assert!(h.events.lock().unwrap().is_empty());
        let updated = invoke(&h, "update_settings", json!({ "patch": { "bufferSeconds": 45 } })).unwrap();
        assert_eq!(*h.events.lock().unwrap(), [updated]);
        // Setting what is already set announces nothing.
        invoke(&h, "update_settings", json!({ "patch": { "bufferSeconds": 45 } })).unwrap();
        assert_eq!(h.events.lock().unwrap().len(), 1);
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
            h.backend.registered(),
            ["Ctrl+Shift+R", "Ctrl+Alt+Backspace", "Ctrl+Alt+Enter"]
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
        assert!(h.events.lock().unwrap().is_empty());
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
}
