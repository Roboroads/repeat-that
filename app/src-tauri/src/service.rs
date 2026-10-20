//! The settings the app is running with: loaded from the file, changed through [`SettingsService::update`], saved,
//! and pushed to the hotkeys and the OS autostart entry.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, RunEvent, Runtime};
use tauri_plugin_store::{Store, StoreExt};

use crate::actions;
use crate::autostart::{Autostart, OsAutostart, reconcile};
use crate::hotkeys::{HotkeyManager, PluginBackend, ShortcutBackend};
use crate::ipc::{CommandError, HotkeyStates, Settings, SettingsPatch};
use crate::settings::{apply_patch, sanitize};

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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Name of the settings file in the app config dir.
pub const SETTINGS_FILE: &str = "settings.json";

/// The settings file: `settings.json` in the app config dir, as an absolute path.
///
/// Open the store with this path and no other. `tauri-plugin-store` resolves a relative path such as `"settings.json"`
/// against the app *data* dir, which is a different folder from the config dir on Linux (`~/.local/share` against
/// `~/.config`), and it keys its open stores by the resolved path: a second `store("settings.json")` anywhere would
/// open another file next to this one. The settings window can't reach the store plugin either (its capability grants
/// no `store:` permission), so [`StorePersistence`] built from this path is the only way in.
pub fn settings_file<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(SETTINGS_FILE))
        .map_err(|err| err.to_string())
}
/// Key of the settings object in that file.
const SETTINGS_KEY: &str = "settings";

/// [`SettingsPersistence`] on `tauri-plugin-store`: the settings object under the `settings` key of a JSON file. A file
/// that can't be read counts as empty and is overwritten by the next save.
pub struct StorePersistence<R: Runtime> {
    store: Arc<Store<R>>,
}

impl<R: Runtime> StorePersistence<R> {
    /// Opens (and reads) the file at `path`, which must be absolute: take it from [`settings_file`].
    pub fn open(app: &AppHandle<R>, path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err(format!(
                "{} is relative, and the store plugin would resolve it against the app data dir",
                path.display()
            ));
        }
        set_aside_corrupt_file(path);
        // Saved explicitly with the result checked, not in the background.
        let store = app
            .store_builder(path)
            .disable_auto_save()
            .build()
            .map_err(|err| err.to_string())?;
        Ok(StorePersistence { store })
    }
}

/// A settings file that can be read but is not a JSON object is moved to `settings.json.bad` (replacing an older
/// backup), so the store starts from nothing, the defaults apply, and the next save writes a fresh file without
/// throwing away what the user had. A missing, empty or unreadable file is left alone.
fn set_aside_corrupt_file(path: &Path) {
    let Ok(bytes) = std::fs::read(path) else { return };
    if bytes.trim_ascii().is_empty() || serde_json::from_slice::<serde_json::Map<String, Value>>(&bytes).is_ok() {
        return;
    }
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bad");
    match std::fs::rename(path, &backup) {
        Ok(()) => eprintln!(
            "the settings file is not valid JSON; kept as {} and started from the defaults",
            Path::new(&backup).display()
        ),
        Err(err) => eprintln!("the settings file is not valid JSON and could not be set aside: {err}"),
    }
}

impl<R: Runtime> SettingsPersistence for StorePersistence<R> {
    fn load(&self) -> Option<Value> {
        self.store.get(SETTINGS_KEY)
    }

    fn save(&self, settings: &Settings) -> Result<(), String> {
        let value = serde_json::to_value(settings).map_err(|err| err.to_string())?;
        self.store.set(SETTINGS_KEY, value);
        self.store.save().map_err(|err| err.to_string())
    }
}

/// Settings that live as long as the app does. Stands in when the settings file can't be set up.
#[derive(Default)]
struct InMemoryPersistence {
    stored: Mutex<Option<Value>>,
}

impl SettingsPersistence for InMemoryPersistence {
    fn load(&self) -> Option<Value> {
        lock(&self.stored).clone()
    }

    fn save(&self, settings: &Settings) -> Result<(), String> {
        *lock(&self.stored) = serde_json::to_value(settings).ok();
        Ok(())
    }
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
fn end_first_run_on_exit<R: Runtime>(app: &AppHandle<R>, event: &RunEvent) {
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
            s.backend.registered(),
            ["Ctrl+Shift+R", "Ctrl+Alt+Backspace", "Ctrl+Alt+Enter"]
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

/// The settings file, on the real store plugin and the mock runtime, in a directory of its own under the system temp
/// dir. Nothing here touches audio or the user's real settings.
#[cfg(test)]
mod store_tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};

    use super::*;
    use crate::autostart::fake::FakeAutostart;
    use crate::hotkeys::fake::FakeBackend;

    /// A fresh directory that is removed again on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "repeat-that-settings-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&dir);
            TempDir(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join(SETTINGS_FILE)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A new mock app, as a new run of the real one would be.
    fn app() -> tauri::App<MockRuntime> {
        mock_builder()
            .plugin(tauri_plugin_store::Builder::new().build())
            .build(mock_context(noop_assets()))
            .expect("mock app builds")
    }

    fn open(app: &tauri::App<MockRuntime>, dir: &TempDir) -> StorePersistence<MockRuntime> {
        StorePersistence::open(app.handle(), &dir.file()).expect("store opens")
    }

    fn settings() -> Settings {
        Settings {
            buffer_seconds: 75,
            first_run_done: true,
            output_device: Some("Headphones".into()),
            ..Settings::default()
        }
    }

    #[test]
    fn the_settings_file_is_an_absolute_path_in_the_config_dir() {
        let app = app();
        let path = settings_file(app.handle()).unwrap();
        assert!(path.is_absolute(), "{path:?}");
        assert_eq!(path.file_name().unwrap(), SETTINGS_FILE);
        assert_eq!(path.parent().unwrap(), app.path().app_config_dir().unwrap());
    }

    #[test]
    fn a_relative_path_is_refused_because_the_store_would_put_it_in_the_data_dir() {
        let app = app();
        assert!(StorePersistence::open(app.handle(), Path::new(SETTINGS_FILE)).is_err());
    }

    #[test]
    fn opening_the_same_path_twice_gives_the_same_store() {
        // The store plugin keys its open stores by path, so every access through the one path shares one store.
        let dir = TempDir::new();
        let app = app();
        let first = open(&app, &dir);
        let second = open(&app, &dir);
        assert!(Arc::ptr_eq(&first.store, &second.store));
    }

    #[test]
    fn a_missing_file_loads_as_nothing() {
        let dir = TempDir::new();
        assert_eq!(open(&app(), &dir).load(), None);
    }

    #[test]
    fn saved_settings_are_read_back_by_the_next_run() {
        let dir = TempDir::new();
        {
            let app = app();
            open(&app, &dir).save(&settings()).unwrap();
        }
        let app = app();
        let stored = open(&app, &dir).load().expect("the settings were stored");
        assert_eq!(sanitize(&stored), settings());
    }

    #[test]
    fn the_settings_live_under_one_key_of_a_json_file() {
        let dir = TempDir::new();
        open(&app(), &dir).save(&settings()).unwrap();
        let file: Value = serde_json::from_str(&fs::read_to_string(dir.file()).unwrap()).unwrap();
        assert_eq!(file, json!({ "settings": serde_json::to_value(settings()).unwrap() }));
        assert_eq!(file["settings"]["firstRunDone"], true);
    }

    #[test]
    fn other_keys_in_the_file_survive_a_save() {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.file(), r#"{ "other": 7 }"#).unwrap();
        open(&app(), &dir).save(&settings()).unwrap();
        let file: Value = serde_json::from_str(&fs::read_to_string(dir.file()).unwrap()).unwrap();
        assert_eq!(file["other"], 7);
        assert_eq!(file["settings"]["bufferSeconds"], 75);
    }

    #[test]
    fn a_corrupt_file_means_defaults_until_the_next_save_replaces_it() {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.file(), "{ this is not json").unwrap();
        let app = app();
        let persistence = open(&app, &dir);
        assert_eq!(persistence.load(), None);
        assert_eq!(sanitize(&Value::Null), Settings::default());

        persistence.save(&settings()).unwrap();
        let file: Value = serde_json::from_str(&fs::read_to_string(dir.file()).unwrap()).unwrap();
        assert_eq!(file["settings"]["bufferSeconds"], 75);
    }

    fn backup_of(dir: &TempDir) -> PathBuf {
        dir.0.join(format!("{SETTINGS_FILE}.bad"))
    }

    #[test]
    fn a_corrupt_file_is_set_aside_so_the_user_keeps_a_copy() {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.file(), "{ this is not json").unwrap();
        let app = app();
        let persistence = open(&app, &dir);
        assert!(!dir.file().exists(), "the corrupt file is moved away");
        assert_eq!(fs::read_to_string(backup_of(&dir)).unwrap(), "{ this is not json");
        assert_eq!(persistence.load(), None);

        persistence.save(&settings()).unwrap();
        assert!(dir.file().exists());
        assert!(backup_of(&dir).exists(), "saving doesn't touch the backup");
    }

    #[test]
    fn a_file_that_is_json_but_not_an_object_is_set_aside_too() {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.file(), "[1, 2, 3]").unwrap();
        let app = app();
        assert_eq!(open(&app, &dir).load(), None);
        assert_eq!(fs::read_to_string(backup_of(&dir)).unwrap(), "[1, 2, 3]");
    }

    #[test]
    fn a_newer_corrupt_file_replaces_the_older_backup() {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(backup_of(&dir), "old").unwrap();
        fs::write(dir.file(), "new {").unwrap();
        let app = app();
        open(&app, &dir);
        assert_eq!(fs::read_to_string(backup_of(&dir)).unwrap(), "new {");
    }

    #[test]
    fn a_readable_file_stays_where_it_is() {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.file(), r#"{ "other": 7 }"#).unwrap();
        let app = app();
        open(&app, &dir);
        assert!(dir.file().exists());
        assert!(!backup_of(&dir).exists());
    }

    #[test]
    fn an_empty_file_is_not_worth_a_backup() {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.file(), "  \n").unwrap();
        let app = app();
        assert_eq!(open(&app, &dir).load(), None);
        assert!(!backup_of(&dir).exists());
    }

    #[test]
    fn a_save_creates_the_missing_folder() {
        let dir = TempDir::new();
        assert!(!dir.0.exists());
        open(&app(), &dir).save(&Settings::default()).unwrap();
        assert!(dir.file().exists());
    }

    #[test]
    fn a_file_that_cannot_be_written_is_an_error_not_a_panic() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.file()).unwrap(); // a folder where the file should go
        let app = app();
        let persistence = open(&app, &dir);
        assert!(persistence.save(&Settings::default()).is_err());
    }

    #[test]
    fn the_app_exiting_remembers_the_first_run_across_runs() {
        let dir = TempDir::new();
        let start = |app: &tauri::App<MockRuntime>| {
            install_with(
                app.handle(),
                Arc::new(open(app, &dir)),
                Arc::new(FakeAutostart::default()),
                Box::new(Arc::new(FakeBackend::default())),
            );
        };
        {
            let app = app();
            start(&app);
            let settings = app.state::<SettingsService>();
            settings.begin_first_run_session();
            // Other run events change nothing; only the exit does.
            end_first_run_on_exit(app.handle(), &RunEvent::Ready);
            assert!(!settings.get().first_run_done);
            end_first_run_on_exit(app.handle(), &RunEvent::Exit);
            assert!(settings.get().first_run_done);
        }
        let app = app();
        start(&app);
        assert!(app.state::<SettingsService>().get().first_run_done);
    }

    #[test]
    fn the_service_runs_on_the_defaults_over_a_corrupt_file_and_rewrites_it_at_the_next_save() {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.file(), "\u{0}\u{0} garbage").unwrap();
        let app = app();
        let service = SettingsService::start(
            Arc::new(open(&app, &dir)),
            Arc::new(FakeAutostart::default()),
            Arc::new(HotkeyManager::new(
                Box::new(Arc::new(FakeBackend::default())),
                Arc::new(|_| {}),
            )),
            Box::new(|_| {}),
        );
        assert_eq!(service.get(), Settings::default());
        service.update(json!({ "bufferSeconds": 50 })).unwrap();
        let file: Value = serde_json::from_str(&fs::read_to_string(dir.file()).unwrap()).unwrap();
        assert_eq!(file["settings"]["bufferSeconds"], 50);
    }

    #[test]
    fn the_service_keeps_its_changes_across_runs() {
        let dir = TempDir::new();
        let start = |app: &tauri::App<MockRuntime>| {
            SettingsService::start(
                Arc::new(open(app, &dir)),
                Arc::new(FakeAutostart::default()),
                Arc::new(HotkeyManager::new(
                    Box::new(Arc::new(FakeBackend::default())),
                    Arc::new(|_| {}),
                )),
                Box::new(|_| {}),
            )
        };
        {
            let app = app();
            let service = start(&app);
            service
                .update(json!({ "bufferSeconds": 90, "hotkeys": { "stop": null }, "firstRunDone": true }))
                .unwrap();
        }
        let app = app();
        let settings = start(&app).get();
        assert_eq!(settings.buffer_seconds, 90);
        assert_eq!(settings.hotkeys.stop, None);
        assert!(settings.first_run_done);
    }
}
