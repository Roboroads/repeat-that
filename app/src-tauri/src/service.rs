//! The settings the app is running with: loaded from the file, changed through [`SettingsService::update`], saved,
//! and pushed to the hotkeys and the OS autostart entry.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, RunEvent, Runtime};

use crate::actions;
use crate::autostart::{Autostart, OsAutostart, reconcile};
use crate::hotkeys::{HotkeyManager, PluginBackend, ShortcutBackend};
use crate::ipc::{CommandError, HotkeyStates, Settings, SettingsPatch};
use crate::settings::{apply_patch, sanitize};
use crate::store::{InMemoryPersistence, StorePersistence, settings_file};

/// Where settings are kept between runs.
pub trait SettingsPersistence: Send + Sync {
    /// The stored settings object; `None` when there is none or the file can't be read.
    fn load(&self) -> Option<Value>;
    fn save(&self, settings: &Settings) -> Result<(), String>;
}

/// Called with the new settings after every change.
pub type ChangeListener = Box<dyn Fn(&Settings) + Send + Sync>;

pub struct SettingsService {
    current: Mutex<Settings>,
    /// Held for a whole update, so two updates can't interleave their side effects. Readers never wait for it.
    writer: Mutex<()>,
    persistence: Arc<dyn SettingsPersistence>,
    autostart: Arc<dyn Autostart>,
    hotkeys: Arc<HotkeyManager>,
    on_change: ChangeListener,
    /// Whether this run opened the settings window because it is the first run.
    first_run_window: AtomicBool,
}

impl SettingsService {
    /// Loads the stored settings, lets the OS autostart entry correct the stored flag and registers the hotkeys.
    pub fn start(
        persistence: Arc<dyn SettingsPersistence>,
        autostart: Arc<dyn Autostart>,
        hotkeys: Arc<HotkeyManager>,
        on_change: ChangeListener,
    ) -> Self {
        let mut settings = persistence.load().map(|stored| sanitize(&stored)).unwrap_or_default();

        let reconciled = reconcile(settings.autostart, autostart.is_enabled());
        if reconciled != settings.autostart {
            settings.autostart = reconciled;
            if let Err(err) = persistence.save(&settings) {
                eprintln!("could not save the settings file: {err}");
            }
        }

        hotkeys.apply(&settings.hotkeys);
        SettingsService {
            current: Mutex::new(settings),
            writer: Mutex::new(()),
            persistence,
            autostart,
            hotkeys,
            on_change,
            first_run_window: AtomicBool::new(false),
        }
    }

    pub fn get(&self) -> Settings {
        lock(&self.current).clone()
    }

    /// Applies `patch` (see [`SettingsPatch`]): validates it, sets the autostart entry, saves, registers changed
    /// hotkeys and tells the listener. If anything fails nothing has changed.
    ///
    /// Runs the hotkey registration, which hops to the main thread: call it from there or from a thread the main thread
    /// doesn't wait on.
    pub fn update(&self, patch: Value) -> Result<Settings, CommandError> {
        let patch: SettingsPatch = serde_json::from_value(patch)
            .map_err(|err| CommandError::invalid(format!("Not a valid settings update: {err}")))?;

        let _writing = lock(&self.writer);
        let current = self.get();
        let next = apply_patch(&current, patch)?;
        if next == current {
            return Ok(next);
        }

        let autostart_changed = next.autostart != current.autostart;
        if autostart_changed {
            self.autostart
                .set_enabled(next.autostart)
                .map_err(|err| CommandError::backend(format!("Could not change starting with the system: {err}")))?;
        }
        if let Err(err) = self.persistence.save(&next) {
            if autostart_changed {
                // Put the entry back so the OS and the settings still agree.
                let _ = self.autostart.set_enabled(current.autostart);
            }
            return Err(CommandError::backend(format!("Could not save the settings: {err}")));
        }

        *lock(&self.current) = next.clone();
        if next.hotkeys != current.hotkeys {
            self.hotkeys.apply(&next.hotkeys);
        }
        (self.on_change)(&next);
        Ok(next)
    }

    /// The first run ends when the user picks an app or closes the window, and at the latest when the app ends
    /// ([`end_first_run_session`](Self::end_first_run_session)).
    pub fn mark_first_run_done(&self) {
        if self.get().first_run_done {
            return;
        }
        if let Err(err) = self.update(json!({ "firstRunDone": true })) {
            eprintln!("could not remember the first run: {err}");
        }
    }

    /// This run opened the settings window because it is the first run.
    pub fn begin_first_run_session(&self) {
        self.first_run_window.store(true, Ordering::SeqCst);
    }

    /// The app is ending. If this run showed the first-run window, the first run is over now, even when the window was
    /// never closed (Quit in the tray, a logoff) and the next launch should start in the tray.
    pub fn end_first_run_session(&self) {
        if self.first_run_window.swap(false, Ordering::SeqCst) {
            self.mark_first_run_done();
        }
    }

    /// See [`HotkeyManager::set_suspended`].
    pub fn suspend_hotkeys(&self, suspended: bool) {
        self.hotkeys.set_suspended(suspended);
    }

    /// Where each hotkey stands, for `Status.hotkeys`.
    // Read by the status once the audio engine fills in the rest of it; tested until then.
    #[allow(dead_code)]
    pub fn hotkey_states(&self) -> HotkeyStates {
        self.hotkeys.states()
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Creates the settings service on the real plugins and manages it. Needs the store, autostart and global-shortcut
/// plugins, and [`SharedEngine`](crate::actions::SharedEngine) in the managed state before a hotkey is pressed.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    let persistence: Arc<dyn SettingsPersistence> =
        match settings_file(app).and_then(|path| StorePersistence::open(app, &path)) {
            Ok(store) => Arc::new(store),
            Err(err) => {
                eprintln!("settings will not be saved: {err}");
                Arc::new(InMemoryPersistence::default())
            }
        };
    install_with(
        app,
        persistence,
        Arc::new(OsAutostart(app.clone())),
        Box::new(PluginBackend(app.clone())),
    );
}

/// [`install`] on the given storage, autostart entry and shortcut backend.
pub fn install_with<R: Runtime>(
    app: &AppHandle<R>,
    persistence: Arc<dyn SettingsPersistence>,
    autostart: Arc<dyn Autostart>,
    backend: Box<dyn ShortcutBackend>,
) {
    let host = app.clone();
    let hotkeys = Arc::new(HotkeyManager::new(
        backend,
        Arc::new(move |action| actions::dispatch(action, &host)),
    ));
    let emitter = app.clone();
    let service = SettingsService::start(
        persistence,
        autostart,
        hotkeys,
        Box::new(move |settings| {
            if let Err(err) = emitter.emit(SETTINGS_CHANGED_EVENT, settings) {
                eprintln!("could not announce the new settings: {err}");
            }
        }),
    );
    app.manage(service);

    // The settings window's close button is not the only way the first run ends: Quit in the tray and a logoff skip it.
    // A plugin hears the app's run events by itself, so nothing has to forward them.
    let lifecycle = tauri::plugin::Builder::<R>::new("settings-lifecycle")
        .on_event(end_first_run_on_exit)
        .build();
    if let Err(err) = app.plugin(lifecycle) {
        eprintln!("could not watch for the app ending: {err}");
    }
}

/// On the app's exit, ends the first run if this run opened the first-run window.
pub(crate) fn end_first_run_on_exit<R: Runtime>(app: &AppHandle<R>, event: &RunEvent) {
    if matches!(event, RunEvent::Exit)
        && let Some(settings) = app.try_state::<SettingsService>()
    {
        settings.end_first_run_session();
    }
}

/// The event that carries the new [`Settings`] after every change.
pub const SETTINGS_CHANGED_EVENT: &str = "settings-changed";

/// Persistence in memory that records its saves and can be told to fail, for tests of the service and the commands.
#[cfg(test)]
pub mod fake {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[derive(Default)]
    pub struct MemoryPersistence {
        pub stored: Mutex<Option<Value>>,
        pub saves: Mutex<Vec<Settings>>,
        failing: AtomicBool,
    }

    impl MemoryPersistence {
        pub fn holding(value: Value) -> Self {
            let memory = MemoryPersistence::default();
            *memory.stored.lock().unwrap() = Some(value);
            memory
        }

        pub fn saves(&self) -> Vec<Settings> {
            self.saves.lock().unwrap().clone()
        }

        pub fn fail_saves(&self, fail: bool) {
            self.failing.store(fail, Ordering::SeqCst);
        }
    }

    impl SettingsPersistence for MemoryPersistence {
        fn load(&self) -> Option<Value> {
            self.stored.lock().unwrap().clone()
        }

        fn save(&self, settings: &Settings) -> Result<(), String> {
            if self.failing.load(Ordering::SeqCst) {
                return Err("disk full".into());
            }
            self.saves.lock().unwrap().push(settings.clone());
            *self.stored.lock().unwrap() = Some(serde_json::to_value(settings).unwrap());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::MemoryPersistence;
    use super::*;
    use crate::actions::AppAction;
    use crate::autostart::fake::FakeAutostart;
    use crate::hotkeys::KeyEdge;
    use crate::hotkeys::fake::FakeBackend;
    use crate::ipc::{CommandErrorKind, HotkeyBindings, HotkeyState, Sensitivity};

    struct Setup {
        service: SettingsService,
        persistence: Arc<MemoryPersistence>,
        autostart: Arc<FakeAutostart>,
        backend: Arc<FakeBackend>,
        actions: Arc<Mutex<Vec<AppAction>>>,
        changes: Arc<Mutex<Vec<Settings>>>,
    }

    fn setup_with(persistence: MemoryPersistence, autostart: FakeAutostart) -> Setup {
        let persistence = Arc::new(persistence);
        let autostart = Arc::new(autostart);
        let backend = Arc::new(FakeBackend::default());
        let actions = Arc::new(Mutex::new(Vec::new()));
        let changes = Arc::new(Mutex::new(Vec::new()));
        let action_sink = Arc::clone(&actions);
        let hotkeys = Arc::new(HotkeyManager::new(
            Box::new(Arc::clone(&backend)),
            Arc::new(move |action| action_sink.lock().unwrap().push(action)),
        ));
        let change_sink = Arc::clone(&changes);
        let service = SettingsService::start(
            persistence.clone(),
            autostart.clone(),
            hotkeys,
            Box::new(move |settings| change_sink.lock().unwrap().push(settings.clone())),
        );
        Setup {
            service,
            persistence,
            autostart,
            backend,
            actions,
            changes,
        }
    }

    fn setup() -> Setup {
        setup_with(MemoryPersistence::default(), FakeAutostart::default())
    }

    // start

    #[test]
    fn a_first_launch_runs_on_the_defaults_and_writes_nothing() {
        let s = setup();
        assert_eq!(s.service.get(), Settings::default());
        assert!(s.persistence.saves().is_empty());
        assert!(s.changes.lock().unwrap().is_empty());
    }

    #[test]
    fn start_registers_the_hotkeys() {
        let s = setup();
        assert_eq!(
            s.backend.registered(),
            ["Ctrl+Alt+Space", "Ctrl+Alt+Backspace", "Ctrl+Alt+Enter"]
        );
        let ok = HotkeyState::Ok;
        assert_eq!(
            (
                s.service.hotkey_states().replay,
                s.service.hotkey_states().stop,
                s.service.hotkey_states().catch_up
            ),
            (ok, ok, ok)
        );
    }

    #[test]
    fn start_reads_the_stored_settings_field_by_field() {
        let s = setup_with(
            MemoryPersistence::holding(json!({
                "bufferSeconds": 45, "sensitivity": "high", "replaySpeed": "fast", "firstRunDone": true,
                "hotkeys": { "replay": "Ctrl+Shift+R" }
            })),
            FakeAutostart::default(),
        );
        let settings = s.service.get();
        assert_eq!(settings.buffer_seconds, 45);
        assert_eq!(settings.sensitivity, Sensitivity::High);
        assert_eq!(settings.replay_speed, 1.0);
        assert!(settings.first_run_done);
        assert_eq!(s.backend.registered()[0], "Ctrl+Shift+R");
    }

    #[test]
    fn a_stored_value_that_is_not_an_object_means_the_defaults() {
        let s = setup_with(MemoryPersistence::holding(json!("garbage")), FakeAutostart::default());
        assert_eq!(s.service.get(), Settings::default());
    }

    #[test]
    fn an_autostart_entry_added_outside_the_app_wins_and_is_saved() {
        let s = setup_with(MemoryPersistence::default(), FakeAutostart::with_enabled(true));
        assert!(s.service.get().autostart);
        assert_eq!(s.persistence.saves().len(), 1);
        assert!(s.persistence.saves()[0].autostart);
    }

    #[test]
    fn an_autostart_entry_removed_outside_the_app_wins_and_is_saved() {
        let s = setup_with(
            MemoryPersistence::holding(json!({ "autostart": true })),
            FakeAutostart::with_enabled(false),
        );
        assert!(!s.service.get().autostart);
        assert_eq!(s.persistence.saves().len(), 1);
        assert!(!s.persistence.saves()[0].autostart);
    }

    #[test]
    fn matching_autostart_state_writes_nothing() {
        let s = setup_with(
            MemoryPersistence::holding(json!({ "autostart": true })),
            FakeAutostart::with_enabled(true),
        );
        assert!(s.service.get().autostart);
        assert!(s.persistence.saves().is_empty());
    }

    #[test]
    fn an_unreadable_autostart_entry_keeps_the_stored_flag() {
        let autostart = FakeAutostart::with_enabled(false);
        *autostart.fail_reads.lock().unwrap() = true;
        let s = setup_with(MemoryPersistence::holding(json!({ "autostart": true })), autostart);
        assert!(s.service.get().autostart);
        assert!(s.persistence.saves().is_empty());
    }

    // update

    #[test]
    fn update_merges_saves_notifies_and_returns_the_new_settings() {
        let s = setup();
        let returned = s
            .service
            .update(json!({ "bufferSeconds": 60, "duck": { "level": 0.5 } }))
            .unwrap();
        assert_eq!(returned.buffer_seconds, 60);
        assert_eq!(returned.duck.level, 0.5);
        assert!(returned.duck.enabled, "the rest of duck is kept");
        assert_eq!(s.service.get(), returned);
        assert_eq!(s.persistence.saves(), std::slice::from_ref(&returned));
        assert_eq!(*s.changes.lock().unwrap(), [returned]);
    }

    #[test]
    fn updates_build_on_each_other() {
        let s = setup();
        s.service.update(json!({ "bufferSeconds": 60 })).unwrap();
        let settings = s.service.update(json!({ "sensitivity": "low" })).unwrap();
        assert_eq!(settings.buffer_seconds, 60);
        assert_eq!(settings.sensitivity, Sensitivity::Low);
        assert_eq!(s.changes.lock().unwrap().len(), 2);
    }

    #[test]
    fn update_clamps() {
        let s = setup();
        assert_eq!(
            s.service
                .update(json!({ "bufferSeconds": 9999 }))
                .unwrap()
                .buffer_seconds,
            120
        );
    }

    #[test]
    fn what_is_saved_loads_back_the_same() {
        let s = setup();
        s.service
            .update(json!({
                "target": { "exePath": "C:\\Discord.exe", "name": "Discord" },
                "outputDevice": "Headphones",
                "hotkeys": { "stop": null },
                "firstRunDone": true
            }))
            .unwrap();
        let reloaded = setup_with(
            MemoryPersistence::holding(s.persistence.stored.lock().unwrap().clone().unwrap()),
            FakeAutostart::default(),
        );
        assert_eq!(reloaded.service.get(), s.service.get());
    }

    #[test]
    fn a_patch_that_changes_nothing_writes_and_notifies_nothing() {
        let s = setup();
        s.service
            .update(json!({ "bufferSeconds": 30, "hotkeys": { "replay": "Ctrl+Alt+Space" } }))
            .unwrap();
        s.service.update(json!({})).unwrap();
        assert!(s.persistence.saves().is_empty());
        assert!(s.changes.lock().unwrap().is_empty());
        assert!(s.autostart.writes.lock().unwrap().is_empty());
    }

    #[test]
    fn a_bad_patch_is_invalid_and_changes_nothing() {
        let s = setup();
        for bad in [
            json!(null),
            json!("x"),
            json!([1]),
            json!({ "bufferSeconds": "lots" }),
            json!({ "sensitivity": 3 }),
        ] {
            let err = s.service.update(bad.clone()).unwrap_err();
            assert_eq!(err.kind, CommandErrorKind::Invalid, "{bad}");
        }
        assert_eq!(s.service.get(), Settings::default());
        assert!(s.persistence.saves().is_empty());
        assert!(s.changes.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unparseable_hotkey_changes_nothing() {
        let s = setup();
        s.backend.clear_log();
        let err = s
            .service
            .update(json!({ "bufferSeconds": 99, "hotkeys": { "replay": "Ctrl+Alt+Frobnicate" } }))
            .unwrap_err();
        assert_eq!(err.kind, CommandErrorKind::Invalid);
        assert_eq!(s.service.get(), Settings::default());
        assert!(s.persistence.saves().is_empty());
        assert!(s.changes.lock().unwrap().is_empty());
        assert!(s.backend.log().is_empty(), "the registered hotkeys were left alone");
    }

    #[test]
    fn a_conflicting_hotkey_changes_nothing() {
        let s = setup();
        s.backend.clear_log();
        let err = s
            .service
            .update(json!({ "autostart": true, "hotkeys": { "stop": "Ctrl+Alt+Space" } }))
            .unwrap_err();
        assert_eq!(err.kind, CommandErrorKind::Conflict);
        assert_eq!(s.service.get(), Settings::default());
        assert!(s.persistence.saves().is_empty());
        assert!(
            s.autostart.writes.lock().unwrap().is_empty(),
            "autostart is not touched for a rejected patch"
        );
        assert!(s.backend.log().is_empty());
    }

    #[test]
    fn changed_hotkeys_are_registered_and_the_old_ones_stop_working() {
        let s = setup();
        s.service
            .update(json!({ "hotkeys": { "replay": "Ctrl+Shift+R" } }))
            .unwrap();
        assert_eq!(
            s.backend.registered_sorted(),
            ["Ctrl+Alt+Backspace", "Ctrl+Alt+Enter", "Ctrl+Shift+R"]
        );
        s.backend.send("Ctrl+Shift+R", KeyEdge::Pressed);
        assert_eq!(*s.actions.lock().unwrap(), [AppAction::Replay]);
    }

    #[test]
    fn other_changes_leave_the_hotkeys_registered_as_they_are() {
        let s = setup();
        s.backend.clear_log();
        s.service
            .update(json!({ "bufferSeconds": 60, "sensitivity": "low" }))
            .unwrap();
        assert!(s.backend.log().is_empty());
    }

    #[test]
    fn a_hotkey_another_app_holds_is_saved_and_reported_in_use() {
        let s = setup();
        s.backend.refuse("Ctrl+Shift+R");
        let settings = s
            .service
            .update(json!({ "hotkeys": { "replay": "Ctrl+Shift+R" } }))
            .unwrap();
        assert_eq!(settings.hotkeys.replay.as_deref(), Some("Ctrl+Shift+R"));
        assert_eq!(s.service.hotkey_states().replay, HotkeyState::InUse);
        assert_eq!(s.service.hotkey_states().stop, HotkeyState::Ok);
    }

    #[test]
    fn unbinding_a_hotkey_unregisters_it() {
        let s = setup();
        s.service.update(json!({ "hotkeys": { "catchUp": null } })).unwrap();
        assert_eq!(s.backend.registered(), ["Ctrl+Alt+Space", "Ctrl+Alt+Backspace"]);
        assert_eq!(s.service.hotkey_states().catch_up, HotkeyState::Unbound);
    }

    #[test]
    fn hotkeys_changed_while_suspended_take_effect_on_resume() {
        let s = setup();
        s.service.suspend_hotkeys(true);
        assert!(s.backend.registered().is_empty());
        s.service
            .update(json!({ "hotkeys": { "replay": "Ctrl+Shift+R" } }))
            .unwrap();
        assert!(s.backend.registered().is_empty());
        s.service.suspend_hotkeys(false);
        assert_eq!(
            s.backend.registered(),
            ["Ctrl+Shift+R", "Ctrl+Alt+Backspace", "Ctrl+Alt+Enter"]
        );
    }

    #[test]
    fn autostart_is_switched_with_the_setting() {
        let s = setup();
        s.service.update(json!({ "autostart": true })).unwrap();
        assert!(*s.autostart.enabled.lock().unwrap());
        s.service.update(json!({ "autostart": false })).unwrap();
        assert!(!*s.autostart.enabled.lock().unwrap());
        assert_eq!(*s.autostart.writes.lock().unwrap(), [true, false]);
    }

    #[test]
    fn a_failing_autostart_entry_is_a_backend_error_and_changes_nothing() {
        let s = setup();
        *s.autostart.fail_writes.lock().unwrap() = true;
        let err = s
            .service
            .update(json!({ "autostart": true, "bufferSeconds": 60 }))
            .unwrap_err();
        assert_eq!(err.kind, CommandErrorKind::Backend);
        assert!(err.message.contains("read-only"), "{}", err.message);
        assert_eq!(s.service.get(), Settings::default());
        assert!(s.persistence.saves().is_empty());
        assert!(s.changes.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failing_save_is_a_backend_error_and_changes_nothing() {
        let s = setup();
        s.persistence.fail_saves(true);
        s.backend.clear_log();
        let err = s
            .service
            .update(json!({ "autostart": true, "hotkeys": { "replay": "Ctrl+Shift+R" } }))
            .unwrap_err();
        assert_eq!(err.kind, CommandErrorKind::Backend);
        assert!(err.message.contains("disk full"), "{}", err.message);
        assert_eq!(s.service.get(), Settings::default());
        assert!(s.changes.lock().unwrap().is_empty());
        assert!(
            s.backend.log().is_empty(),
            "hotkeys are only registered once the settings are saved"
        );
        assert!(!*s.autostart.enabled.lock().unwrap(), "the autostart entry is put back");
    }

    #[test]
    fn updates_work_again_after_a_failure() {
        let s = setup();
        s.persistence.fail_saves(true);
        assert!(s.service.update(json!({ "bufferSeconds": 60 })).is_err());
        s.persistence.fail_saves(false);
        assert_eq!(
            s.service.update(json!({ "bufferSeconds": 60 })).unwrap().buffer_seconds,
            60
        );
    }

    // first run

    #[test]
    fn marking_the_first_run_done_saves_and_notifies_once() {
        let s = setup();
        s.service.mark_first_run_done();
        assert!(s.service.get().first_run_done);
        assert_eq!(s.persistence.saves().len(), 1);
        assert_eq!(s.changes.lock().unwrap().len(), 1);
        s.service.mark_first_run_done();
        assert_eq!(s.persistence.saves().len(), 1, "already done: nothing to write");
        assert_eq!(s.changes.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_app_ending_during_the_first_run_session_ends_the_first_run() {
        // Quit in the tray and a logoff never close the window, so this is what remembers the first run.
        let s = setup();
        s.service.begin_first_run_session();
        s.service.end_first_run_session();
        assert!(s.service.get().first_run_done);
        assert_eq!(s.persistence.saves().len(), 1);
        assert_eq!(s.changes.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_session_that_never_showed_the_first_run_window_leaves_the_first_run_alone() {
        let s = setup();
        s.service.end_first_run_session();
        assert!(!s.service.get().first_run_done);
        assert!(s.persistence.saves().is_empty());
    }

    #[test]
    fn ending_the_session_after_the_first_run_was_already_done_writes_nothing() {
        let s = setup();
        s.service.begin_first_run_session();
        s.service.mark_first_run_done();
        s.service.end_first_run_session();
        assert_eq!(s.persistence.saves().len(), 1);
    }

    #[test]
    fn marking_the_first_run_done_keeps_the_other_settings() {
        let s = setup_with(
            MemoryPersistence::holding(json!({ "bufferSeconds": 45 })),
            FakeAutostart::default(),
        );
        s.service.mark_first_run_done();
        assert_eq!(s.service.get().buffer_seconds, 45);
    }

    #[test]
    fn hotkey_bindings_are_kept_as_typed_but_trimmed() {
        let s = setup();
        let settings = s
            .service
            .update(json!({ "hotkeys": { "replay": "  ctrl+shift+r " } }))
            .unwrap();
        assert_eq!(
            settings.hotkeys,
            HotkeyBindings {
                replay: Some("ctrl+shift+r".into()),
                ..HotkeyBindings::default()
            }
        );
    }
}
