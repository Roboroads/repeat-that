//! The audio engine inside the app: started at launch with the stored settings, kept in step with every settings change,
//! and the source of the status the settings window and the tray show.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rt_capture::AppAudioSource;
use rt_engine::{Engine, EngineConfig, EngineHandle, OutputBackend, TargetApp};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::actions::EngineControl;
use crate::hotkeys::HotkeyReport;
use crate::ipc::{
    CaptureStateName, CaptureStatus, DesktopStatus, HotkeyBackendKind, HotkeyStates, OutputStatus, Playback,
    Sensitivity, Settings, Status,
};
use crate::locking::lock;
use crate::service::SettingsService;
use crate::session::{self, Env, ProcessEnv, SharedSession};
use crate::source::{self, SourceKind};
use crate::tray::{self, HotkeyLabels, SharedStatus, TrayState, TrayStatus};

/// The event that carries the [`Status`] after every change, and every 500 ms during a replay.
pub const STATUS_EVENT: &str = "status";

/// What the engine runs with for `settings`.
pub fn engine_config(settings: &Settings) -> EngineConfig {
    EngineConfig {
        buffer: Duration::from_secs(u64::from(settings.buffer_seconds)),
        sensitivity: match settings.sensitivity {
            Sensitivity::Low => rt_engine::Sensitivity::Low,
            Sensitivity::Normal => rt_engine::Sensitivity::Normal,
            Sensitivity::High => rt_engine::Sensitivity::High,
        },
        replay_speed: settings.replay_speed as f32,
        catch_up_speed: settings.catch_up_speed as f32,
        duck: settings.duck.enabled.then_some(settings.duck.level as f32),
        output_device: settings.output_device.clone(),
    }
}

/// The app the engine captures for `settings`.
pub fn engine_target(settings: &Settings) -> Option<TargetApp> {
    settings.target.as_ref().map(|target| TargetApp {
        exe_path: target.exe_path.as_ref().map(PathBuf::from),
        name: target.name.clone(),
    })
}

/// The engine's status as the window and the tray get it, with what the settings know (the hotkeys and whether the
/// settings can be saved) and what the desktop is like.
pub fn wire_status(
    engine: &rt_engine::Status,
    hotkeys: HotkeyStates,
    settings_problem: Option<String>,
    desktop: DesktopStatus,
) -> Status {
    use rt_engine::CaptureState as Engine;
    let (state, message) = match &engine.capture {
        Engine::NoTarget => (CaptureStateName::NoTarget, None),
        Engine::Waiting => (CaptureStateName::Waiting, None),
        Engine::Capturing => (CaptureStateName::Capturing, None),
        Engine::Paused => (CaptureStateName::Paused, None),
        Engine::Error(message) => (CaptureStateName::Error, Some(message.clone())),
        Engine::Unsupported(message) => (CaptureStateName::Unsupported, Some(message.clone())),
    };
    Status {
        capture: CaptureStatus { state, message },
        target_name: engine.target_name.clone(),
        playback: match engine.playback {
            rt_engine::PlaybackState::Idle => Playback::Idle,
            rt_engine::PlaybackState::Replaying => Playback::Replaying,
            rt_engine::PlaybackState::CatchingUp => Playback::CatchingUp,
        },
        behind_ms: engine.behind.map(millis),
        buffered_ms: millis(engine.buffered),
        output: OutputStatus {
            device_name: engine.output.device_name.clone(),
            fell_back: engine.output.fell_back,
            error: engine.output.error.clone(),
        },
        hotkeys,
        vad_fallback: engine.vad_fallback,
        settings_problem,
        desktop,
    }
}

/// What the status says about the desktop: how the hotkeys work (and the keys the desktop gave, or the commands to bind
/// by hand when there are none) and whether the tray has a host.
pub fn desktop_status(report: HotkeyReport, tray_available: bool, env: &dyn Env) -> DesktopStatus {
    DesktopStatus {
        hotkey_backend: report.backend,
        hotkey_triggers: report.triggers,
        hotkey_commands: (report.backend == HotkeyBackendKind::None).then(|| session::hotkey_commands(env)),
        tray_available,
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The running engine, and what it was told. Before [`run`](Self::run) and after [`shutdown`](Self::shutdown) there
/// is no engine and every call does nothing, except that a pause is remembered for when it runs.
#[derive(Default)]
pub struct AppEngine {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    engine: Option<Engine>,
    paused: bool,
    /// The config and target the engine has, so a settings change that doesn't touch them sends nothing.
    config: Option<EngineConfig>,
    target: Option<Option<TargetApp>>,
}

/// The engine as managed app state.
pub type SharedAppEngine = Arc<AppEngine>;

impl AppEngine {
    /// Starts the engine on `source` and `output` with `settings`. It captures at once unless a pause came first:
    /// pausing isn't stored, so every launch captures.
    pub fn run(&self, source: Arc<dyn AppAudioSource>, output: Box<dyn OutputBackend>, settings: &Settings) {
        let config = engine_config(settings);
        let engine = Engine::new(source, output, config.clone());
        let mut inner = lock(&self.inner);
        if inner.paused {
            engine.handle().set_paused(true);
        }
        inner.engine = Some(engine);
        inner.config = Some(config);
        inner.target = None;
        drop(inner);
        self.apply(settings);
    }

    /// A handle on the running engine.
    pub fn handle(&self) -> Option<EngineHandle> {
        lock(&self.inner).engine.as_ref().map(Engine::handle)
    }

    /// Gives the engine what `settings` ask of it: only what changed since it was last told.
    pub fn apply(&self, settings: &Settings) {
        let mut inner = lock(&self.inner);
        let Some(handle) = inner.engine.as_ref().map(Engine::handle) else {
            return;
        };
        let config = engine_config(settings);
        if inner.config.as_ref() != Some(&config) {
            handle.set_config(config.clone());
            inner.config = Some(config);
        }
        let target = engine_target(settings);
        if inner.target.as_ref() != Some(&target) {
            handle.set_target(target.clone());
            inner.target = Some(target);
        }
    }

    /// Whether pause was asked for last. The status says whether the engine has paused.
    #[cfg(test)]
    pub fn is_paused(&self) -> bool {
        lock(&self.inner).paused
    }

    /// Pauses or resumes: `change` gets whether pause was asked for last and returns what is wanted now. One step, so
    /// two toggles at once end up back where they started. Pausing stops capture and wipes the buffer. Returns whether
    /// pause is asked for now.
    pub fn change_paused(&self, change: impl FnOnce(bool) -> bool) -> bool {
        let mut inner = lock(&self.inner);
        inner.paused = change(inner.paused);
        if let Some(engine) = &inner.engine {
            engine.handle().set_paused(inner.paused);
        }
        inner.paused
    }

    /// The engine's status; the status of an engine that has nothing to do when there is none.
    pub fn status(&self) -> rt_engine::Status {
        self.handle().map(|handle| handle.status()).unwrap_or_default()
    }

    /// Receives every change of the engine's status until the engine stops.
    pub fn subscribe(&self) -> Option<Receiver<rt_engine::Status>> {
        self.handle().map(|handle| handle.subscribe())
    }

    /// Stops the engine: playback and capture end, the buffer is wiped and the captured app gets its volume back. Waits
    /// for the engine's threads, which takes a fade at most.
    pub fn shutdown(&self) {
        let engine = lock(&self.inner).engine.take();
        // Dropped outside the lock: stopping waits for the engine's threads.
        drop(engine);
    }

    /// The config the engine was given last.
    #[cfg(test)]
    pub fn config(&self) -> Option<EngineConfig> {
        lock(&self.inner).config.clone()
    }
}

impl EngineControl for AppEngine {
    fn replay(&self) {
        if let Some(handle) = self.handle() {
            handle.replay();
        }
    }

    fn stop(&self) {
        if let Some(handle) = self.handle() {
            handle.stop();
        }
    }

    fn toggle_catch_up(&self) {
        if let Some(handle) = self.handle() {
            handle.toggle_catch_up();
        }
    }
}

/// Starts the engine on this OS's capture source and output, see [`source`]. Needs the [`SettingsService`],
/// [`SharedAppEngine`] and [`SharedStatus`] in the managed state.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    start_publishing(app);
    let kind = source::from_env();
    // Only the OS backend lowers volumes, so only it needs somewhere to note them.
    let journal = if kind == SourceKind::Platform {
        duck_journal(app)
    } else {
        None
    };
    install_with(app, source::build(kind, journal), source::output_from_env());
}

/// [`install`] on the given source and output, without starting the [`StatusPublisher`]. The status goes out on every
/// change of the engine's.
pub fn install_with<R: Runtime>(app: &AppHandle<R>, source: Arc<dyn AppAudioSource>, output: Box<dyn OutputBackend>) {
    let engine = Arc::clone(&app.state::<SharedAppEngine>());
    engine.run(source, output, &app.state::<SettingsService>().get());
    let mut changes = engine.subscribe();
    let app = app.clone();
    // Ends when the engine stops. The engine drops a subscriber that falls far behind, so until then this one subscribes
    // again whenever its channel ends.
    let forward = move || {
        while let Some(subscription) = changes.take().or_else(|| engine.subscribe()) {
            while subscription.recv().is_ok() {
                // A publish sends the status as it is now, so changes queued meanwhile need no publish of their own.
                while subscription.try_recv().is_ok() {}
                publish_status(&app);
            }
        }
    };
    if let Err(err) = std::thread::Builder::new().name("rt-status".into()).spawn(forward) {
        eprintln!("could not start forwarding the status: {err}");
    }
}

/// Where the OS capture backend notes the volumes it lowered: `duck.journal` in the app's local data directory.
/// It holds volumes and audio session identifiers only, never audio.
fn duck_journal<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let dir = match app.path().app_local_data_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("no place for the duck journal: {err}");
            return None;
        }
    };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("could not create {}: {err}", dir.display());
        return None;
    }
    Some(dir.join("duck.journal"))
}

/// Pauses or resumes capture, see [`AppEngine::change_paused`]: the one way the tray, the hotkeys and the window do it.
/// The engine's status changes once it has paused, and that goes out like any other change.
pub fn change_paused<R: Runtime>(app: &AppHandle<R>, change: impl FnOnce(bool) -> bool) {
    if let Some(engine) = app.try_state::<SharedAppEngine>() {
        engine.change_paused(change);
    }
}

/// The settings changed: the engine follows them, and the status goes out again because the hotkeys may have changed.
pub fn settings_changed<R: Runtime>(app: &AppHandle<R>, settings: &Settings) {
    if let Some(engine) = app.try_state::<SharedAppEngine>() {
        engine.apply(settings);
    }
    publish_status(app);
}

/// The status as it is now.
pub fn current_status<R: Runtime>(app: &AppHandle<R>) -> Status {
    let engine = app
        .try_state::<SharedAppEngine>()
        .map(|engine| engine.status())
        .unwrap_or_default();
    let (hotkeys, problem, report) = app
        .try_state::<SettingsService>()
        .map(|settings| {
            (
                settings.hotkey_states(),
                settings.storage_problem(),
                settings.hotkey_report(),
            )
        })
        .unwrap_or_default();
    // Where the app doesn't look at the desktop (tests), there is a tray.
    let tray_available = app
        .try_state::<SharedSession>()
        .is_none_or(|session| session.tray_available());
    wire_status(
        &engine,
        hotkeys,
        problem,
        desktop_status(report, tray_available, &ProcessEnv),
    )
}

/// Sends the status to the window and the tray, always from a thread of its own. So statuses go out in order, nothing
/// is locked while the window's listeners run (they may change the settings or pause, which sends the status again),
/// and asking for a send never waits.
pub struct StatusPublisher {
    wake: Sender<Wake>,
}

enum Wake {
    Publish,
    Stop,
}

/// Starts the [`StatusPublisher`] and sends the first status. Until it runs, asking for a send does nothing.
pub fn start_publishing<R: Runtime>(app: &AppHandle<R>) {
    let (wake, wakes) = mpsc::channel();
    let publisher = app.clone();
    let publish = move || {
        while let Ok(Wake::Publish) = wakes.recv() {
            // A send has the status as it is now, so the requests queued meanwhile are covered by it.
            let mut stop = false;
            while let Ok(wake) = wakes.try_recv() {
                stop |= matches!(wake, Wake::Stop);
            }
            if stop {
                break;
            }
            send_status(&publisher);
        }
    };
    if let Err(err) = std::thread::Builder::new().name("rt-publish".into()).spawn(publish) {
        eprintln!("could not start sending the status: {err}");
        return;
    }
    app.manage(StatusPublisher { wake });
    publish_status(app);
}

/// Has the current status sent to the window and shown in the tray. Returns at once.
pub fn publish_status<R: Runtime>(app: &AppHandle<R>) {
    if let Some(publisher) = app.try_state::<StatusPublisher>() {
        let _ = publisher.wake.send(Wake::Publish);
    }
}

/// The body of [`publish_status`], on the publisher's thread.
fn send_status<R: Runtime>(app: &AppHandle<R>) {
    let status = current_status(app);
    if let Err(err) = app.emit(STATUS_EVENT, &status) {
        eprintln!("could not send the status: {err}");
    }
    let hotkeys = app
        .try_state::<SettingsService>()
        .map(|settings| HotkeyLabels::for_status(&settings.get().hotkeys, &status))
        .unwrap_or_default();
    if let Some(shared) = app.try_state::<SharedStatus>() {
        *tray::lock_status(&shared) = TrayState {
            status: TrayStatus::from_status(&status),
            hotkeys,
        };
    }
    tray::refresh(app);
}

/// Stops the engine when the app ends, see [`AppEngine::shutdown`], and then the [`StatusPublisher`].
pub fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    if let Some(engine) = app.try_state::<SharedAppEngine>() {
        engine.shutdown();
    }
    if let Some(publisher) = app.try_state::<StatusPublisher>() {
        let _ = publisher.wake.send(Wake::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{Duck, HotkeyState, HotkeyTriggers, Target};

    #[test]
    fn the_engine_config_follows_the_settings() {
        let settings = Settings {
            buffer_seconds: 45,
            sensitivity: Sensitivity::High,
            replay_speed: 0.75,
            catch_up_speed: 2.0,
            duck: Duck {
                enabled: true,
                level: 0.35,
            },
            output_device: Some("headphones".into()),
            ..Settings::default()
        };
        assert_eq!(
            engine_config(&settings),
            EngineConfig {
                buffer: Duration::from_secs(45),
                sensitivity: rt_engine::Sensitivity::High,
                replay_speed: 0.75,
                catch_up_speed: 2.0,
                duck: Some(0.35),
                output_device: Some("headphones".into()),
            }
        );
    }

    #[test]
    fn the_default_settings_are_the_engine_defaults() {
        assert_eq!(engine_config(&Settings::default()), EngineConfig::default());
    }

    #[test]
    fn ducking_off_leaves_the_volume_alone() {
        let settings = Settings {
            duck: Duck {
                enabled: false,
                level: 0.2,
            },
            ..Settings::default()
        };
        assert_eq!(engine_config(&settings).duck, None);
    }

    #[test]
    fn each_sensitivity_maps_to_its_own() {
        let sensitivity = |sensitivity| {
            engine_config(&Settings {
                sensitivity,
                ..Settings::default()
            })
            .sensitivity
        };
        assert_eq!(sensitivity(Sensitivity::Low), rt_engine::Sensitivity::Low);
        assert_eq!(sensitivity(Sensitivity::Normal), rt_engine::Sensitivity::Normal);
        assert_eq!(sensitivity(Sensitivity::High), rt_engine::Sensitivity::High);
    }

    #[test]
    fn the_target_is_matched_by_path_and_by_name_without_one() {
        let target = |exe_path: Option<&str>| {
            engine_target(&Settings {
                target: Some(Target {
                    exe_path: exe_path.map(String::from),
                    name: "Discord".into(),
                }),
                ..Settings::default()
            })
        };
        assert_eq!(
            target(Some("C:\\Discord.exe")),
            Some(TargetApp {
                exe_path: Some(PathBuf::from("C:\\Discord.exe")),
                name: "Discord".into(),
            })
        );
        assert_eq!(
            target(None),
            Some(TargetApp {
                exe_path: None,
                name: "Discord".into(),
            })
        );
        assert_eq!(engine_target(&Settings::default()), None);
    }

    fn ok_hotkeys() -> HotkeyStates {
        HotkeyStates {
            replay: HotkeyState::Ok,
            stop: HotkeyState::InUse,
            catch_up: HotkeyState::Unbound,
        }
    }

    #[test]
    fn the_wire_status_carries_the_engine_status() {
        let engine = rt_engine::Status {
            capture: rt_engine::CaptureState::Capturing,
            target_name: Some("Discord".into()),
            playback: rt_engine::PlaybackState::CatchingUp,
            behind: Some(Duration::from_millis(3250)),
            buffered: Duration::from_millis(24_500),
            output: rt_engine::OutputStatus {
                device_name: Some("Speakers".into()),
                fell_back: true,
                error: None,
            },
            vad_fallback: true,
        };
        assert_eq!(
            wire_status(&engine, ok_hotkeys(), None, DesktopStatus::default()),
            Status {
                capture: CaptureStatus {
                    state: CaptureStateName::Capturing,
                    message: None,
                },
                target_name: Some("Discord".into()),
                playback: Playback::CatchingUp,
                behind_ms: Some(3250),
                buffered_ms: 24_500,
                output: OutputStatus {
                    device_name: Some("Speakers".into()),
                    fell_back: true,
                    error: None,
                },
                hotkeys: ok_hotkeys(),
                vad_fallback: true,
                settings_problem: None,
                desktop: DesktopStatus::default(),
            }
        );
    }

    #[test]
    fn every_capture_state_has_its_name_and_message() {
        use rt_engine::CaptureState as E;
        let wire = |capture| {
            wire_status(
                &rt_engine::Status {
                    capture,
                    ..Default::default()
                },
                ok_hotkeys(),
                None,
                DesktopStatus::default(),
            )
            .capture
        };
        let expect = |state, message: Option<&str>| CaptureStatus {
            state,
            message: message.map(String::from),
        };
        assert_eq!(wire(E::NoTarget), expect(CaptureStateName::NoTarget, None));
        assert_eq!(wire(E::Waiting), expect(CaptureStateName::Waiting, None));
        assert_eq!(wire(E::Capturing), expect(CaptureStateName::Capturing, None));
        assert_eq!(wire(E::Paused), expect(CaptureStateName::Paused, None));
        assert_eq!(
            wire(E::Error("access denied".into())),
            expect(CaptureStateName::Error, Some("access denied"))
        );
        assert_eq!(
            wire(E::Unsupported("no backend".into())),
            expect(CaptureStateName::Unsupported, Some("no backend"))
        );
    }

    #[test]
    fn idle_playback_is_not_behind() {
        let status = wire_status(
            &rt_engine::Status::default(),
            HotkeyStates::UNBOUND,
            None,
            DesktopStatus::default(),
        );
        assert_eq!(status.playback, Playback::Idle);
        assert_eq!(status.behind_ms, None);
        assert_eq!(status.buffered_ms, 0);
    }

    #[test]
    fn a_settings_problem_reaches_the_status() {
        let status = wire_status(
            &rt_engine::Status::default(),
            HotkeyStates::UNBOUND,
            Some("The settings file is read-only".into()),
            DesktopStatus::default(),
        );
        assert_eq!(
            status.settings_problem.as_deref(),
            Some("The settings file is read-only")
        );
    }

    fn wire_json(desktop: DesktopStatus) -> serde_json::Value {
        serde_json::to_value(wire_status(&rt_engine::Status::default(), ok_hotkeys(), None, desktop)).unwrap()
    }

    #[test]
    fn the_status_is_camel_case_on_the_wire() {
        assert_eq!(
            wire_json(DesktopStatus::default()),
            serde_json::json!({
                "capture": { "state": "noTarget", "message": null },
                "targetName": null,
                "playback": "idle",
                "behindMs": null,
                "bufferedMs": 0,
                "output": { "deviceName": null, "fellBack": false, "error": null },
                "hotkeys": { "replay": "ok", "stop": "inUse", "catchUp": "unbound" },
                "vadFallback": false,
                "settingsProblem": null,
                "hotkeyBackend": "plugin",
                "hotkeyTriggers": { "replay": null, "stop": null, "catchUp": null },
                "hotkeyCommands": null,
                "trayAvailable": true,
            })
        );
    }

    #[test]
    fn the_portal_triggers_and_a_missing_tray_reach_the_status() {
        let json = wire_json(DesktopStatus {
            hotkey_backend: HotkeyBackendKind::Portal,
            hotkey_triggers: HotkeyTriggers {
                replay: Some("Ctrl+Alt+Space".into()),
                stop: None,
                catch_up: Some("Ctrl+Alt+Enter".into()),
            },
            hotkey_commands: None,
            tray_available: false,
        });
        assert_eq!(json["hotkeyBackend"], "portal");
        assert_eq!(
            json["hotkeyTriggers"],
            serde_json::json!({ "replay": "Ctrl+Alt+Space", "stop": null, "catchUp": "Ctrl+Alt+Enter" })
        );
        assert_eq!(json["hotkeyCommands"], serde_json::Value::Null);
        assert_eq!(json["trayAvailable"], false);
    }

    #[test]
    fn the_commands_reach_the_status_when_there_are_no_hotkeys() {
        let json = wire_json(DesktopStatus {
            hotkey_backend: HotkeyBackendKind::None,
            hotkey_commands: Some(crate::session::hotkey_commands(&crate::session::ProcessEnv)),
            ..DesktopStatus::default()
        });
        assert_eq!(json["hotkeyBackend"], "none");
        let commands = &json["hotkeyCommands"];
        assert!(
            commands["replay"].as_str().unwrap().ends_with(" --replay"),
            "{commands}"
        );
        assert!(commands["stop"].as_str().unwrap().ends_with(" --stop"), "{commands}");
        assert!(
            commands["catchUp"].as_str().unwrap().ends_with(" --catch-up"),
            "{commands}"
        );
    }

    struct FlatpakEnv;

    impl Env for FlatpakEnv {
        fn var(&self, name: &str) -> Option<String> {
            (name == "FLATPAK_ID").then(|| "me.roboroads.RepeatThat".to_string())
        }
    }

    fn report(backend: HotkeyBackendKind, replay: Option<&str>) -> HotkeyReport {
        HotkeyReport {
            backend,
            triggers: HotkeyTriggers {
                replay: replay.map(String::from),
                ..HotkeyTriggers::default()
            },
        }
    }

    #[test]
    fn with_the_plugin_the_desktop_has_nothing_to_add() {
        assert_eq!(
            desktop_status(report(HotkeyBackendKind::Plugin, None), true, &FlatpakEnv),
            DesktopStatus::default()
        );
    }

    #[test]
    fn under_the_portal_the_status_carries_the_keys_and_no_commands() {
        let desktop = desktop_status(
            report(HotkeyBackendKind::Portal, Some("Ctrl+Alt+Space")),
            true,
            &FlatpakEnv,
        );
        assert_eq!(desktop.hotkey_backend, HotkeyBackendKind::Portal);
        assert_eq!(desktop.hotkey_triggers.replay.as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(desktop.hotkey_commands, None);
    }

    #[test]
    fn with_no_hotkeys_the_status_carries_the_commands_of_this_package() {
        let desktop = desktop_status(report(HotkeyBackendKind::None, None), true, &FlatpakEnv);
        assert_eq!(desktop.hotkey_backend, HotkeyBackendKind::None);
        let commands = desktop.hotkey_commands.expect("the commands to bind by hand");
        assert_eq!(commands.replay, "flatpak run me.roboroads.RepeatThat --replay");
        assert_eq!(commands.stop, "flatpak run me.roboroads.RepeatThat --stop");
        assert_eq!(commands.catch_up, "flatpak run me.roboroads.RepeatThat --catch-up");
    }

    #[test]
    fn a_missing_tray_host_is_in_the_status() {
        assert!(!desktop_status(report(HotkeyBackendKind::Plugin, None), false, &FlatpakEnv).tray_available);
        assert!(desktop_status(report(HotkeyBackendKind::Plugin, None), true, &FlatpakEnv).tray_available);
    }

    #[test]
    fn the_existing_status_fields_keep_their_names() {
        // The desktop's fields sit next to them, not in a group of their own: what the window already reads must not move.
        let json = wire_json(DesktopStatus::default());
        for key in [
            "capture",
            "targetName",
            "playback",
            "behindMs",
            "bufferedMs",
            "output",
            "hotkeys",
            "vadFallback",
            "settingsProblem",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert!(json.get("desktop").is_none());
    }

    #[test]
    fn without_an_engine_nothing_happens_but_a_pause_is_remembered() {
        let engine = AppEngine::default();
        engine.replay();
        engine.stop();
        engine.toggle_catch_up();
        engine.apply(&Settings::default());
        assert!(engine.handle().is_none());
        assert_eq!(engine.status(), rt_engine::Status::default());
        assert!(engine.change_paused(|paused| !paused));
        assert!(engine.is_paused());
        assert!(!engine.change_paused(|paused| !paused));
        engine.change_paused(|_| true);
        assert!(engine.is_paused());
        engine.shutdown();
    }
}
