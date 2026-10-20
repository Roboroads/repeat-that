//! Tests on tauri's mock runtime. On Windows the test executable needs the same Common Controls v6 manifest as the app
//! (see `build.rs`); without it the executable doesn't even start (`STATUS_ENTRYPOINT_NOT_FOUND`) as soon as it links
//! code that uses `TaskDialogIndirect`.

use std::sync::Mutex;

use tauri::test::{mock_builder, mock_context, noop_assets};

use tauri::Manager;

use crate::actions::{AppAction, dispatch};
use crate::tray::{SharedStatus, TrayStatus, lock_status};

#[test]
fn toggle_pause_works_on_the_mock_runtime() {
    let app = mock_builder()
        .manage::<SharedStatus>(Mutex::new(TrayStatus::default()))
        .build(mock_context(noop_assets()))
        .expect("the mock app builds");
    let handle = app.handle();
    let is_paused = || lock_status(&handle.state::<SharedStatus>()).is_paused();

    assert!(!is_paused());
    dispatch(AppAction::TogglePause, handle);
    assert!(is_paused());
    dispatch(AppAction::TogglePause, handle);
    assert!(!is_paused());
}

/// Pause toggles from the tray and the hotkeys can arrive at the same time; an even number of toggles must always end
/// where it started, which a read followed by a separate write would not guarantee.
#[test]
fn simultaneous_pause_toggles_do_not_get_lost() {
    let app = mock_builder()
        .manage::<SharedStatus>(Mutex::new(TrayStatus::default()))
        .build(mock_context(noop_assets()))
        .expect("the mock app builds");

    let threads: Vec<_> = (0..4)
        .map(|_| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                for _ in 0..5000 {
                    dispatch(AppAction::TogglePause, &handle);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("the toggling thread finishes");
    }

    assert!(!lock_status(&app.handle().state::<SharedStatus>()).is_paused());
}

/// The manifest is what makes Windows load Common Controls v6, the only version that has `TaskDialogIndirect`
/// (tauri's `common-controls-v6` feature links to it). Asking for the function proves the test executable got the
/// manifest, whatever the linker decided to keep.
#[cfg(windows)]
#[test]
fn test_executable_loads_common_controls_v6() {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryA(name: *const u8) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    // SAFETY: both strings are NUL-terminated, and comctl32.dll is a system DLL that is safe to load.
    let function = unsafe {
        let module = LoadLibraryA(c"comctl32.dll".as_ptr().cast());
        assert!(!module.is_null(), "comctl32.dll did not load");
        GetProcAddress(module, c"TaskDialogIndirect".as_ptr().cast())
    };
    assert!(
        !function.is_null(),
        "comctl32.dll is version 5: the test executable has no Common Controls v6 manifest"
    );
}
