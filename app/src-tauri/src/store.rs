//! The settings file: where `tauri-plugin-store` keeps it, how a damaged one is handled, and the in-memory stand-in for
//! when no file can be set up.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_store::{Store, StoreExt};

use crate::ipc::{SETTINGS_VERSION, Settings};
use crate::service::SettingsPersistence;

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

/// [`SettingsPersistence`] on `tauri-plugin-store`: the settings object under the `settings` key of a JSON file.
///
/// A file that holds something else than a JSON object is moved aside (see [`set_aside_corrupt_file`]). A file that
/// can't be read, or can't be moved aside, is never written over: saving is refused until it can be dealt with, so
/// the empty store the plugin opens in its place can't replace what the user had. Neither is a file that a newer
/// version of the app wrote. Saving a file this version does understand keeps the fields it doesn't know.
pub struct StorePersistence<R: Runtime> {
    store: Arc<Store<R>>,
    path: PathBuf,
    /// Why saving is refused, while it is.
    blocked: Mutex<Option<Block>>,
}

/// Why a [`StorePersistence`] refuses to save.
enum Block {
    /// The file can't be read or moved aside; looked at again before each save.
    Unreadable(String),
    /// A newer version of the app wrote the file; stays so while this version runs.
    Newer(String),
}

impl Block {
    fn message(&self) -> &str {
        match self {
            Block::Unreadable(message) | Block::Newer(message) => message,
        }
    }
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
        let mut blocked = match set_aside_corrupt_file(path) {
            Aside::Failed(reason) => Some(Block::Unreadable(reason)),
            Aside::Nothing | Aside::Moved => None,
        };
        // Saved explicitly with the result checked, not in the background.
        let store = app
            .store_builder(path)
            .disable_auto_save()
            .build()
            .map_err(|err| err.to_string())?;
        if blocked.is_none()
            && let Some(version) = stored_version(store.get(SETTINGS_KEY).as_ref())
            && version > u64::from(SETTINGS_VERSION)
        {
            blocked = Some(Block::Newer(format!(
                "The settings were saved by a newer version of Repeat that?! (settings version {version}). This version \
                 reads them but leaves the file alone, so changes can't be saved."
            )));
        }
        if blocked.is_some() {
            // The plugin saves every store it knows when the app exits. This one must not be among them: it holds
            // nothing of a file we couldn't read, and must not rewrite one we must not touch.
            store.close_resource();
        }
        Ok(StorePersistence {
            store,
            path: path.to_path_buf(),
            blocked: Mutex::new(blocked),
        })
    }

    /// Whether saving may go ahead. A file that couldn't be set aside is tried again, since the cause (a lock held by
    /// a virus scanner or a backup tool, say) may be gone by now.
    fn check_writable(&self) -> Result<(), String> {
        let mut blocked = crate::service::lock(&self.blocked);
        if let Some(Block::Unreadable(_)) = *blocked {
            match set_aside_corrupt_file(&self.path) {
                Aside::Failed(reason) => *blocked = Some(Block::Unreadable(reason)),
                Aside::Nothing | Aside::Moved => *blocked = None,
            }
        }
        match &*blocked {
            Some(block) => Err(block.message().to_string()),
            None => Ok(()),
        }
    }
}

/// The `version` of a stored settings object, if it has one.
fn stored_version(stored: Option<&Value>) -> Option<u64> {
    stored?.get("version")?.as_u64()
}

/// `new` laid over `old`: objects merge key by key, so fields that only a newer version knows stay where they were,
/// and anything else in `new` replaces what was there.
fn overlay(old: Option<Value>, new: Value) -> Value {
    match (old, new) {
        (Some(Value::Object(mut old)), Value::Object(new)) => {
            for (key, value) in new {
                let merged = overlay(old.remove(&key), value);
                old.insert(key, merged);
            }
            Value::Object(old)
        }
        (_, new) => new,
    }
}

/// What [`set_aside_corrupt_file`] did.
#[derive(Debug, PartialEq, Eq)]
enum Aside {
    /// There was no file, or it holds a JSON object (or only blanks), or it was set aside before.
    Nothing,
    /// The file held something else than a JSON object and was moved to `settings.json.bad`.
    Moved,
    /// The file can't be read, or can't be moved: it is left as it is. Carries the reason.
    Failed(String),
}

/// A settings file that can be read but is not a JSON object is moved to `settings.json.bad` (replacing an older
/// backup), so the store starts from nothing, the defaults apply, and the next save writes a fresh file without
/// throwing away what the user had. A missing or blank file has nothing to keep. A file that can't be read, or can't
/// be moved, is left alone and reported: writing a fresh file over it would destroy what we couldn't look at.
fn set_aside_corrupt_file(path: &Path) -> Aside {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Aside::Nothing,
        Err(err) => {
            return Aside::Failed(format!(
                "The settings file can't be read, so it is left as it is and changes can't be saved: {err}"
            ));
        }
    };
    if bytes.trim_ascii().is_empty() || serde_json::from_slice::<serde_json::Map<String, Value>>(&bytes).is_ok() {
        return Aside::Nothing;
    }
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bad");
    match std::fs::rename(path, &backup) {
        Ok(()) => {
            eprintln!(
                "the settings file is not valid JSON; kept as {} and started from the defaults",
                Path::new(&backup).display()
            );
            Aside::Moved
        }
        Err(err) => Aside::Failed(format!(
            "The settings file is damaged and can't be moved aside, so it is left as it is and changes can't be saved: {err}"
        )),
    }
}

impl<R: Runtime> SettingsPersistence for StorePersistence<R> {
    fn load(&self) -> Option<Value> {
        self.store.get(SETTINGS_KEY)
    }

    fn save(&self, settings: &Settings) -> Result<(), String> {
        self.check_writable()?;
        let value = serde_json::to_value(settings).map_err(|err| err.to_string())?;
        let previous = self.store.get(SETTINGS_KEY);
        self.store.set(SETTINGS_KEY, overlay(previous.clone(), value));
        if let Err(err) = self.store.save() {
            // The plugin writes every open store again when the app exits, so a change that was refused must not stay
            // in memory to be persisted by that.
            match previous {
                Some(previous) => self.store.set(SETTINGS_KEY, previous),
                None => {
                    self.store.delete(SETTINGS_KEY);
                }
            }
            return Err(err.to_string());
        }
        Ok(())
    }
}

/// Settings that live as long as the app does. Stands in when the settings file can't be set up.
#[derive(Default)]
pub struct InMemoryPersistence {
    stored: Mutex<Option<Value>>,
}

impl SettingsPersistence for InMemoryPersistence {
    fn load(&self) -> Option<Value> {
        crate::service::lock(&self.stored).clone()
    }

    fn save(&self, settings: &Settings) -> Result<(), String> {
        *crate::service::lock(&self.stored) = serde_json::to_value(settings).ok();
        Ok(())
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
    use crate::hotkeys::HotkeyManager;
    use crate::hotkeys::fake::FakeBackend;
    use crate::service::{SettingsService, end_first_run_on_exit, install_with};
    use crate::settings::sanitize;
    use serde_json::json;
    use tauri::RunEvent;

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

    /// A corrupt settings file whose backup name is taken by a non-empty folder, so moving it aside fails.
    fn corrupt_file_that_cannot_be_moved(dir: &TempDir) {
        fs::create_dir_all(backup_of(dir)).unwrap();
        fs::write(backup_of(dir).join("in-the-way"), "x").unwrap();
        fs::write(dir.file(), "{ this is not json").unwrap();
    }

    #[test]
    fn a_corrupt_file_that_cannot_be_moved_aside_is_never_overwritten() {
        let dir = TempDir::new();
        corrupt_file_that_cannot_be_moved(&dir);
        let app = app();
        let persistence = open(&app, &dir);
        assert_eq!(persistence.load(), None);
        assert!(persistence.save(&settings()).is_err());
        assert_eq!(fs::read_to_string(dir.file()).unwrap(), "{ this is not json");
        // The plugin writes every open store when the app exits; this one must not be among them.
        assert!(app.get_store(dir.file()).is_none());
    }

    #[test]
    fn saving_works_again_once_the_corrupt_file_can_be_moved() {
        let dir = TempDir::new();
        corrupt_file_that_cannot_be_moved(&dir);
        let app = app();
        let persistence = open(&app, &dir);
        assert!(persistence.save(&settings()).is_err());

        fs::remove_dir_all(backup_of(&dir)).unwrap(); // whatever was in the way is gone
        persistence.save(&settings()).unwrap();
        assert_eq!(fs::read_to_string(backup_of(&dir)).unwrap(), "{ this is not json");
        let file: Value = serde_json::from_str(&fs::read_to_string(dir.file()).unwrap()).unwrap();
        assert_eq!(file["settings"]["bufferSeconds"], 75);
    }

    fn write_settings_file(dir: &TempDir, content: &Value) {
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.file(), content.to_string()).unwrap();
    }

    fn read_file(dir: &TempDir) -> Value {
        serde_json::from_str(&fs::read_to_string(dir.file()).unwrap()).unwrap()
    }

    #[test]
    fn fields_only_a_newer_version_knows_survive_a_save() {
        let dir = TempDir::new();
        write_settings_file(
            &dir,
            &json!({
                "settings": {
                    "version": 1,
                    "bufferSeconds": 30,
                    "futureThing": { "a": 1 },
                    "duck": { "enabled": true, "level": 0.2, "curve": "log" },
                    "hotkeys": { "replay": "Ctrl+Alt+Space", "mute": "Ctrl+M" }
                },
                "other": 7
            }),
        );
        let app = app();
        open(&app, &dir).save(&settings()).unwrap();
        let file = read_file(&dir);
        assert_eq!(file["settings"]["bufferSeconds"], 75, "known fields are updated");
        assert_eq!(file["settings"]["futureThing"], json!({ "a": 1 }));
        assert_eq!(file["settings"]["duck"]["curve"], "log");
        assert_eq!(file["settings"]["hotkeys"]["mute"], "Ctrl+M");
        assert_eq!(file["other"], 7);
    }

    #[test]
    fn a_cleared_target_replaces_the_stored_one_instead_of_merging_into_it() {
        let dir = TempDir::new();
        write_settings_file(
            &dir,
            &json!({ "settings": { "version": 1, "target": { "exePath": "C:\\a.exe", "name": "A" } } }),
        );
        let app = app();
        open(&app, &dir).save(&Settings::default()).unwrap();
        assert_eq!(read_file(&dir)["settings"]["target"], Value::Null);
    }

    #[test]
    fn a_settings_file_without_a_version_gets_one() {
        let dir = TempDir::new();
        write_settings_file(&dir, &json!({ "settings": { "bufferSeconds": 30 } }));
        let app = app();
        open(&app, &dir).save(&settings()).unwrap();
        assert_eq!(read_file(&dir)["settings"]["version"], 1);
    }

    #[test]
    fn a_settings_value_that_is_not_an_object_is_replaced() {
        let dir = TempDir::new();
        write_settings_file(&dir, &json!({ "settings": "garbage" }));
        let app = app();
        open(&app, &dir).save(&settings()).unwrap();
        assert_eq!(read_file(&dir)["settings"]["bufferSeconds"], 75);
    }

    #[test]
    fn a_file_from_a_newer_version_is_read_but_never_written() {
        let dir = TempDir::new();
        let newer = json!({ "settings": { "version": 2, "bufferSeconds": 45, "shiny": true } });
        write_settings_file(&dir, &newer);
        let before = fs::read_to_string(dir.file()).unwrap();
        let app = app();
        let persistence = open(&app, &dir);
        let stored = persistence.load().expect("the newer file is still read");
        assert_eq!(sanitize(&stored).buffer_seconds, 45);

        let err = persistence.save(&settings()).unwrap_err();
        assert!(err.contains("newer version"), "{err}");
        assert_eq!(fs::read_to_string(dir.file()).unwrap(), before);
        assert!(app.get_store(dir.file()).is_none(), "nor by the plugin's flush at exit");
    }

    #[test]
    fn a_file_that_cannot_be_read_is_left_alone_too() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.file()).unwrap(); // reading a folder fails, but it isn't a missing file
        let app = app();
        let persistence = open(&app, &dir);
        assert!(persistence.save(&settings()).is_err());
        assert!(dir.file().is_dir());
        assert!(app.get_store(dir.file()).is_none());
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
    fn a_failed_write_leaves_no_trace_in_the_store() {
        // The plugin writes every open store when the app exits, so a rejected change must not stay in memory.
        let dir = TempDir::new();
        let app = app();
        let persistence = open(&app, &dir);
        fs::create_dir_all(dir.file()).unwrap(); // a folder where the file should go
        assert!(persistence.save(&settings()).is_err());
        assert_eq!(persistence.load(), None);
    }

    #[test]
    fn a_failed_write_keeps_the_last_good_settings() {
        let dir = TempDir::new();
        let app = app();
        let persistence = open(&app, &dir);
        persistence.save(&Settings::default()).unwrap();
        fs::remove_file(dir.file()).unwrap();
        fs::create_dir_all(dir.file()).unwrap();
        assert!(persistence.save(&settings()).is_err());
        let kept = persistence.load().expect("the earlier settings are still there");
        assert_eq!(sanitize(&kept), Settings::default());
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
