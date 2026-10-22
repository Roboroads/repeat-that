//! The commands behind the window's help for a capture that isn't allowed (`notAllowed`): which buttons it can show,
//! asking the user again and opening the OS settings. They go to [`rt_capture::permission`], which only does something
//! on macOS. Elsewhere there is nothing to allow, and only a debug build's
//! [`MockPermission`](crate::source::MockPermission) answers them.
//!
//! System Settings is opened from here, not through the opener plugin, whose capability stays limited to GitHub.

use rt_capture::CaptureError;
use tauri::{AppHandle, Manager, Runtime};

use crate::commands::off_the_main_thread;
use crate::ipc::{CommandError, PermissionHelp};
use crate::source::SharedMockPermission;

/// The commands that reset the permission or open System Settings on macOS, which a test mustn't run there.
#[cfg(test)]
pub const TOUCHES_THE_OS: [&str; 2] = ["ask_for_capture_permission", "open_capture_permission_settings"];

/// Which of the other two commands work here. Both `false` where the OS doesn't ask for this permission.
#[tauri::command]
pub fn capture_permission_help<R: Runtime>(app: AppHandle<R>) -> Result<PermissionHelp, CommandError> {
    if app.try_state::<SharedMockPermission>().is_some() {
        // It pretends to be macOS 15, which has both.
        return Ok(PermissionHelp {
            can_open_settings: true,
            can_ask_again: true,
        });
    }
    match rt_capture::permission::help() {
        Ok(help) => Ok(PermissionHelp {
            can_open_settings: help.can_open_settings,
            can_ask_again: help.can_ask_again,
        }),
        // Nothing to allow on this OS, or this OS version can't capture at all.
        Err(CaptureError::Unsupported(_)) => Ok(PermissionHelp::default()),
        Err(err) => Err(permission_error(err)),
    }
}

/// Makes the OS ask the user once more. Returns once it asks, not when the user answered: the engine tries the capture
/// again every 2 s, so it starts once they allow it. With the
/// [`MockPermission`](crate::source::MockPermission) the answer is yes, at once.
#[tauri::command]
pub async fn ask_for_capture_permission<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    if let Some(permission) = app.try_state::<SharedMockPermission>() {
        permission.allow();
        return Ok(());
    }
    // Runs `tccutil` on macOS.
    off_the_main_thread(|| rt_capture::permission::ask_again().map_err(permission_error)).await
}

/// Opens the OS settings page where the user allows capture.
#[tauri::command]
pub async fn open_capture_permission_settings() -> Result<(), CommandError> {
    off_the_main_thread(|| rt_capture::permission::open_settings().map_err(permission_error)).await
}

fn permission_error(err: CaptureError) -> CommandError {
    match err {
        // Already sentences for the user.
        CaptureError::Unsupported(why) | CaptureError::Backend(why) | CaptureError::PermissionDenied(why) => {
            CommandError::backend(why)
        }
        other => CommandError::backend(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use rt_capture::MockSource;
    use serde_json::{Value, json};
    use tauri::Manager;

    use crate::engine;
    use crate::source::{MOCK_HOW_TO_ALLOW, MockPermission, SharedMockPermission};
    use crate::test_app::{FIXTURE, TestApp};

    const NO_HELP: fn() -> Value = || json!({ "canOpenSettings": false, "canAskAgain": false });

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_systems_have_nothing_to_allow() {
        let h = TestApp::without_engine();
        assert_eq!(h.invoke("capture_permission_help", json!({})).unwrap(), NO_HELP());
        for cmd in ["ask_for_capture_permission", "open_capture_permission_settings"] {
            let error = h.invoke(cmd, json!({})).unwrap_err();
            assert_eq!(error["kind"], "backend", "{cmd}");
            let message = error["message"].as_str().unwrap();
            assert!(message.contains(std::env::consts::OS), "{cmd}: {message}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_can_always_ask_again() {
        // CI runs macOS 14.2 or later; opening the settings needs 15. The other two commands would touch the runner.
        let h = TestApp::without_engine();
        let help = h.invoke("capture_permission_help", json!({})).unwrap();
        assert_eq!(help["canAskAgain"], true, "{help}");
        assert!(help["canOpenSettings"].is_boolean(), "{help}");
    }

    /// A test app whose engine captures the fixture behind a permission that wasn't given.
    fn not_allowed() -> (TestApp, SharedMockPermission) {
        let h = TestApp::without_engine();
        let permission = MockPermission::denied();
        h.app.manage::<SharedMockPermission>(Arc::clone(&permission));
        let source = permission.gate(Arc::new(MockSource::from_wav(FIXTURE).expect("the fixture plays")));
        engine::install_with(h.app.handle(), source, Box::new(h.output.clone()));
        (h, permission)
    }

    fn state(status: &Value) -> &str {
        status["capture"]["state"].as_str().expect("a capture state")
    }

    #[test]
    fn the_mock_permission_offers_both_buttons_and_ask_again_allows_it() {
        let (h, permission) = not_allowed();
        assert_eq!(
            h.invoke("capture_permission_help", json!({})).unwrap(),
            json!({ "canOpenSettings": true, "canAskAgain": true })
        );
        let app = h.invoke("list_apps", json!({})).unwrap()[0].clone();
        h.invoke("select_app", json!({ "app": app })).unwrap();
        let status = h.wait_for_status(Duration::from_secs(5), "not allowed", |s| state(s) == "notAllowed");
        assert_eq!(status["capture"]["message"], MOCK_HOW_TO_ALLOW);
        assert_eq!(status["targetName"], "speech_8s");

        assert_eq!(h.invoke("ask_for_capture_permission", json!({})).unwrap(), Value::Null);
        assert!(permission.is_allowed());
        // The engine tries again within 2 s.
        h.wait_for_status(Duration::from_secs(5), "capturing", |s| state(s) == "capturing");
    }

    #[test]
    fn without_the_mock_permission_a_capture_is_allowed_as_before() {
        let h = TestApp::with_engine();
        if !cfg!(target_os = "macos") {
            assert_eq!(h.invoke("capture_permission_help", json!({})).unwrap(), NO_HELP());
        }
        let app = h.invoke("list_apps", json!({})).unwrap()[0].clone();
        h.invoke("select_app", json!({ "app": app })).unwrap();
        h.wait_for_status(Duration::from_secs(5), "capturing", |s| state(s) == "capturing");
    }
}
