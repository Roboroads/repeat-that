//! Looking for updates, and installing them where the app can.
//!
//! The wire types here are the contract with the settings window (`UpdateStatus`, the `update-status` event and the
//! three commands in `commands.rs`). The rules live in the submodules and are pure, so they are tested without a
//! network or a window; this file joins them: the [`UpdateService`] with its scheduler thread and install flow, and the
//! glue to the app (the plugin, the tray, the settings).
//!
//! **Off until there is a key.** The updater only exists in a build that has the update public key compiled in
//! (`REPEAT_THAT_UPDATER_PUBKEY`, set by the release workflow from a repository variable). Without it the plugin isn't
//! registered, the mode is `off`, nothing is shown and nothing contacts the network.
//!
//! **Nothing happens without a click.** An update is found and announced; it is downloaded and installed only after the
//! `install_update` command, which the window sends from its "Install and restart" button.

mod machine;
mod mode;
mod notes;
mod schedule;
mod source;

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::engine;
use crate::ipc::{CommandError, Settings};
use crate::locking::lock;
use crate::service::SettingsService;
use crate::session::ProcessEnv;
use crate::tray::{self, SharedStatus, UpdateTrayView};
use machine::Machine;
use mode::{BuildFacts, Detection, Os, SystemFacts};
use schedule::{Attempt, Schedule, Timings};
use source::{MOCK_VAR, MockPace, MockSource, PluginSource};
// The service is made of these, and so are the tests of everything that wants one.
#[cfg(test)]
pub use source::Found;
pub use source::UpdateSource;

pub use mode::UpdateMode;

/// The event that carries the new [`UpdateStatus`] after every change.
pub const STATUS_EVENT: &str = "update-status";

/// Where an update is in its life, `UpdateStatus.state` on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateState {
    /// Nothing has been checked yet.
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Installing,
    Error,
}

/// The step that failed, `UpdateStatus.errorKind` on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateErrorKind {
    /// Asking for the update information.
    Check,
    /// Downloading the update, or installing it.
    Install,
}

/// The update that was found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub version: String,
    /// The day it was released, `YYYY-MM-DD`.
    pub date: Option<String>,
    /// Plain text, already cleaned (see `notes::clean_notes`).
    pub notes: String,
    /// The release page of this version on GitHub.
    pub release_url: String,
}

/// Everything the settings window and the tray show about updates. Sent with the `update-status` event on every change.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub mode: UpdateMode,
    /// Why the mode is not `install`; `None` for `install`.
    pub mode_reason: Option<String>,
    pub current_version: String,
    pub state: UpdateState,
    /// The update that was found. Kept when its download or install fails (the window links to its page), cleared when
    /// a check fails.
    pub available: Option<AvailableUpdate>,
    /// How far the download is, 0 to 1; `None` while its size is unknown.
    pub progress: Option<f64>,
    /// What went wrong, in state `error`.
    pub error: Option<String>,
    /// Which step failed, in state `error`; `None` otherwise. After a failed install `available` is still the update
    /// that failed (its `releaseUrl` is the page to download it from); after a failed check it is `None`.
    pub error_kind: Option<UpdateErrorKind>,
    /// Milliseconds since the epoch; only checks of this run are remembered.
    pub last_checked_at: Option<u64>,
    /// The latest check was asked for by the user.
    pub manual: bool,
}

// What is compiled in

/// The public key that updates must be signed with, set when the app is built. Absent or empty: no updater.
const PUBKEY: Option<&str> = option_env!("REPEAT_THAT_UPDATER_PUBKEY");
/// A different place to read the update information from, for trying the updater against a local server.
const ENDPOINT_OVERRIDE: Option<&str> = option_env!("REPEAT_THAT_UPDATER_ENDPOINT");

/// The update information of this platform, in the latest release. `{{target}}` and `{{arch}}` are the plugin's: they
/// become `windows`, `darwin` or `linux`, and `x86_64` or `aarch64`.
pub const ENDPOINT: &str =
    "https://github.com/Roboroads/repeat-that/releases/latest/download/latest-{{target}}-{{arch}}.json";

/// The key as the plugin wants it (the contents of the `.pub` file that `tauri signer generate` writes, which is base64
/// of two lines of text), or `None` when `raw` is empty or isn't one. A stray newline from a copied file is dropped.
fn clean_pubkey(raw: &str) -> Option<String> {
    let key = raw.trim();
    let text = BASE64.decode(key).ok()?;
    let text = String::from_utf8(text).ok()?;
    text.starts_with("untrusted comment:").then(|| key.to_string())
}

/// The update public key of this build, if it has a usable one.
fn pubkey() -> Option<String> {
    PUBKEY.and_then(clean_pubkey)
}

/// Whether this build can update: the key is what makes the updater exist.
pub fn is_compiled_in() -> bool {
    pubkey().is_some()
}

/// Whether `url` is plain `http` to this machine, the one place that isn't `https` an endpoint may be: a server for
/// trying the updater. The parts after the host are checked so `http://127.0.0.1:80@example.com/` doesn't pass.
fn is_local_http(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let (host, port) = authority
        .rsplit_once(':')
        .map_or((authority, None), |(h, p)| (h, Some(p)));
    matches!(host, "127.0.0.1" | "localhost")
        && port.is_none_or(|port| !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit()))
}

/// The plugin's configuration: the key and where to look. Built here and put into the app's config at startup rather
/// than written in `tauri.conf.json`, so a build without a key has none of it. An `endpoint` that isn't `https` (or
/// local `http`) or that the plugin can't read is not used.
fn plugin_config(pubkey: &str, endpoint: Option<&str>) -> Value {
    let config = |endpoint: &str, insecure: bool| {
        json!({
            "pubkey": pubkey,
            "endpoints": [endpoint],
            "dangerousInsecureTransportProtocol": insecure,
        })
    };
    let wanted = endpoint.map(str::trim).filter(|endpoint| !endpoint.is_empty());
    if let Some(endpoint) = wanted {
        let local = is_local_http(endpoint);
        let custom = config(endpoint, local);
        if (local || endpoint.starts_with("https://"))
            && serde_json::from_value::<tauri_plugin_updater::Config>(custom.clone()).is_ok()
        {
            return custom;
        }
        eprintln!("REPEAT_THAT_UPDATER_ENDPOINT {endpoint:?} is not https, or http to 127.0.0.1 or localhost; ignored");
    }
    config(ENDPOINT, false)
}

/// The updater plugin, when this build has a key; register it with [`configure`].
pub fn plugin<R: Runtime>() -> Option<TauriPlugin<R, tauri_plugin_updater::Config>> {
    pubkey().map(|_| tauri_plugin_updater::Builder::new().build())
}

/// Puts the plugin's configuration into the app's. Does nothing without a key (then there is no plugin to read it).
pub fn configure<R: Runtime>(context: &mut tauri::Context<R>) {
    if let Some(key) = pubkey() {
        context
            .config_mut()
            .plugins
            .0
            .insert("updater".into(), plugin_config(&key, ENDPOINT_OVERRIDE));
    }
}

// The service

/// Time, from the outside: how long the app has run (for the schedule) and the wall clock (for "last checked").
pub trait Clock: Send + Sync {
    fn elapsed(&self) -> Duration;
    fn now_ms(&self) -> u64;
}

pub struct SystemClock {
    started: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        SystemClock {
            started: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
    }
}

/// What the service does to the app around it.
pub trait UpdateHost: Send + Sync {
    /// Shows the new status: to the window and in the tray.
    fn publish(&self, status: &UpdateStatus);
    /// Stops the engine: a replay ends and the app that was ducked gets its volume back. Can be done twice.
    fn shutdown_engine(&self);
    /// Starts the app again (the new version) and ends this run.
    fn restart(&self);
}

/// When the engine stops during an install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownTiming {
    /// Before the installer starts, because it ends the process (Windows: the installer closes the app and starts it
    /// again). If the install then fails, the engine is gone, so the app restarts.
    BeforeInstall,
    /// After the new files are in place (macOS, an AppImage: they are replaced while the app runs), right before the
    /// restart. An install that fails leaves a working app behind.
    AfterInstall,
}

impl ShutdownTiming {
    pub const CURRENT: ShutdownTiming = if cfg!(windows) {
        ShutdownTiming::BeforeInstall
    } else {
        ShutdownTiming::AfterInstall
    };
}

/// How often the download's progress goes out.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Lets one thing through per `every`.
struct Throttle {
    every: Duration,
    last: Option<Duration>,
}

impl Throttle {
    fn new(every: Duration) -> Self {
        Throttle { every, last: None }
    }

    fn ready(&mut self, now: Duration) -> bool {
        if self.last.is_some_and(|last| now < last + self.every) {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// How much of a download has arrived, 0 to 1; `None` while its size is unknown.
fn fraction(received: u64, total: Option<u64>) -> Option<f64> {
    let total = total.filter(|total| *total > 0)?;
    Some((received as f64 / total as f64).min(1.0))
}

/// Everything a service is made of.
pub struct Parts {
    pub detection: Detection,
    pub current_version: String,
    /// `checkForUpdates` as it is stored.
    pub consent: Option<bool>,
    pub source: Arc<dyn UpdateSource>,
    pub host: Arc<dyn UpdateHost>,
    pub clock: Arc<dyn Clock>,
    pub timings: Timings,
    pub shutdown: ShutdownTiming,
}

struct State {
    machine: Machine,
    schedule: Schedule,
    stopped: bool,
}

struct Shared {
    state: Mutex<State>,
    /// Woken when the schedule changes or the service stops.
    wake: Condvar,
    source: Arc<dyn UpdateSource>,
    host: Arc<dyn UpdateHost>,
    clock: Arc<dyn Clock>,
    shutdown: ShutdownTiming,
}

/// Looks for updates and installs them. Cheap to clone: all clones are the same service.
#[derive(Clone)]
pub struct UpdateService {
    shared: Arc<Shared>,
}

impl UpdateService {
    /// Starts the service: shows the first status and, where the app looks for updates, starts the thread that checks
    /// on schedule.
    pub fn start(parts: Parts) -> Self {
        let looks = matches!(parts.detection.mode, UpdateMode::Install | UpdateMode::Notify);
        let machine = Machine::new(parts.detection.mode, parts.detection.reason, &parts.current_version);
        let schedule = Schedule::new(parts.timings, looks, parts.consent, parts.clock.elapsed());
        parts.host.publish(machine.status());
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                machine,
                schedule,
                stopped: false,
            }),
            wake: Condvar::new(),
            source: parts.source,
            host: parts.host,
            clock: parts.clock,
            shutdown: parts.shutdown,
        });
        if looks {
            let scheduled = Arc::clone(&shared);
            if let Err(error) = std::thread::Builder::new()
                .name("rt-update-schedule".into())
                .spawn(move || scheduled.run_schedule())
            {
                eprintln!("could not start checking for updates: {error}");
            }
        }
        UpdateService { shared }
    }

    pub fn status(&self) -> UpdateStatus {
        lock(&self.shared.state).machine.status().clone()
    }

    /// Checks now and waits for the answer. Returns the status as it is at the end, or at once if a check, a download
    /// or an install is already going on, or this install never looks.
    pub fn check(&self, manual: bool) -> UpdateStatus {
        self.shared.check(manual)
    }

    /// Starts downloading and installing the update that was found, on a thread of its own. Fails if there is none to
    /// install or this install can't replace itself. The progress comes with the status events.
    pub fn install(&self) -> Result<(), CommandError> {
        {
            let mut state = lock(&self.shared.state);
            state.machine.begin_download()?;
            self.shared.host.publish(state.machine.status());
        }
        let installing = Arc::clone(&self.shared);
        let started = std::thread::Builder::new()
            .name("rt-update-install".into())
            .spawn(move || installing.run_install());
        if let Err(error) = started {
            eprintln!("could not start the update: {error}");
            self.shared
                .change(|machine| machine.fail_install(source::INSTALL_FAILED.to_string()));
            return Err(CommandError::backend(source::INSTALL_FAILED));
        }
        Ok(())
    }

    /// The user answered the question (or changed the answer).
    pub fn consent_changed(&self, consent: Option<bool>) {
        let mut state = lock(&self.shared.state);
        let now = self.shared.clock.elapsed();
        state.schedule.consent_changed(consent, now);
        self.shared.wake.notify_all();
    }

    /// Stops the scheduler thread. Anything under way finishes.
    pub fn shutdown(&self) {
        lock(&self.shared.state).stopped = true;
        self.shared.wake.notify_all();
    }
}

impl Shared {
    /// Changes the machine and shows the result. The status goes out while the lock is held, so statuses reach the
    /// window in the order they happened; showing one only sends an event and asks the tray to update on the main
    /// thread, neither of which waits for anything.
    fn change<T>(&self, change: impl FnOnce(&mut Machine) -> T) -> T {
        let mut state = lock(&self.state);
        let result = change(&mut state.machine);
        self.host.publish(state.machine.status());
        result
    }

    fn check(&self, manual: bool) -> UpdateStatus {
        {
            let mut state = lock(&self.state);
            if !state.machine.begin_check(manual) {
                // A scheduled check that finds the way blocked is tried again later; one the user asked for just hears
                // how things stand.
                if !manual {
                    let now = self.clock.elapsed();
                    state.schedule.attempted(Attempt::Skipped, now);
                    self.wake.notify_all();
                }
                return state.machine.status().clone();
            }
            self.host.publish(state.machine.status());
        }
        // Asked outside the lock: it waits for the network.
        let outcome = self.source.check();
        let mut state = lock(&self.state);
        state.machine.finish_check(outcome, self.clock.now_ms());
        let status = state.machine.status().clone();
        let worked = status.state != UpdateState::Error;
        let attempt = if manual {
            Attempt::Manual { worked }
        } else {
            Attempt::Automatic { worked }
        };
        let now = self.clock.elapsed();
        state.schedule.attempted(attempt, now);
        self.host.publish(&status);
        self.wake.notify_all();
        status
    }

    /// The body of the install thread: download, then install, then restart.
    fn run_install(&self) {
        let mut throttle = Throttle::new(PROGRESS_EVERY);
        let downloaded = self.source.download(&mut |received, total| {
            if throttle.ready(self.clock.elapsed()) {
                self.change(|machine| machine.set_progress(fraction(received, total)));
            }
        });
        if let Err(message) = downloaded {
            self.change(|machine| machine.fail_install(message));
            return;
        }
        self.change(|machine| machine.begin_install());

        let stop_first = self.shutdown == ShutdownTiming::BeforeInstall;
        if stop_first {
            self.host.shutdown_engine();
        }
        match self.source.install() {
            Ok(()) => {
                if !stop_first {
                    self.host.shutdown_engine();
                }
                self.host.restart();
            }
            Err(message) => {
                self.change(|machine| machine.fail_install(message));
                // The engine is gone and the installer didn't take over: the app would sit there deaf. Starting it again
                // brings back what it was.
                if stop_first {
                    self.host.restart();
                }
            }
        }
    }

    /// The body of the scheduler thread: waits until a check is due, runs it, and waits again.
    fn run_schedule(&self) {
        let mut state = lock(&self.state);
        loop {
            if state.stopped {
                return;
            }
            let now = self.clock.elapsed();
            match state.schedule.next_due() {
                Some(due) if due <= now => {
                    drop(state);
                    self.check(false);
                    state = lock(&self.state);
                }
                Some(due) => {
                    state = self
                        .wake
                        .wait_timeout(state, due - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                None => state = self.wake.wait(state).unwrap_or_else(PoisonError::into_inner),
            }
        }
    }
}

// The app

/// The app as the service's host: statuses go to the window and the tray, and an install ends with the engine stopped and
/// the app started again. `restarts` is off for the mock, which pretends to install and must leave the app alone.
pub struct AppHost<R: Runtime> {
    app: AppHandle<R>,
    restarts: bool,
}

impl<R: Runtime> AppHost<R> {
    pub fn new(app: AppHandle<R>, restarts: bool) -> Self {
        AppHost { app, restarts }
    }
}

impl<R: Runtime> UpdateHost for AppHost<R> {
    fn publish(&self, status: &UpdateStatus) {
        if let Err(error) = self.app.emit(STATUS_EVENT, status) {
            eprintln!("could not send the update status: {error}");
        }
        if let Some(shared) = self.app.try_state::<SharedStatus>() {
            tray::lock_status(&shared).updates = UpdateTrayView::from_status(status);
        }
        tray::refresh(&self.app);
    }

    fn shutdown_engine(&self) {
        if self.restarts {
            engine::shutdown(&self.app);
        }
    }

    fn restart(&self) {
        if self.restarts {
            // From a thread that isn't the main one this asks the app to exit with the restart code and waits for that:
            // the exit runs the exit handlers, the single-instance plugin gives up its name, and only then is the new
            // version started, with the arguments this one had (so `--hidden` stays hidden).
            self.app.restart();
        }
    }
}

/// Starts the updater: works out what this install can do, makes the service and manages it. Needs the settings (for the
/// answer to the question) and [`SharedStatus`] in the managed state, and must come before the tray is set up, so the
/// tray menu starts with its update item.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    let value = std::env::var(MOCK_VAR).ok();
    let mock = source::mock_kind(cfg!(debug_assertions), value.as_deref());
    if cfg!(debug_assertions) && value.is_some() && mock.is_none() {
        eprintln!("{MOCK_VAR} should be available, none, fail-check or fail-download; no mock update source is used");
    }
    let build = BuildFacts {
        has_key: is_compiled_in(),
        debug_build: cfg!(debug_assertions),
        mocked: mock.is_some(),
        os: Os::CURRENT,
    };
    let source: Arc<dyn UpdateSource> = match mock {
        Some(kind) => Arc::new(MockSource::new(kind, MockPace::SLOW)),
        None => Arc::new(PluginSource::new(app.clone())),
    };
    let consent = app
        .try_state::<SettingsService>()
        .and_then(|settings| settings.get().check_for_updates);
    app.manage(UpdateService::start(Parts {
        detection: mode::detect(build, &ProcessEnv, &SystemFacts),
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        consent,
        source,
        host: Arc::new(AppHost::new(app.clone(), mock.is_none())),
        clock: Arc::new(SystemClock::new()),
        timings: Timings::PRODUCTION,
        shutdown: ShutdownTiming::CURRENT,
    }));
}

fn service_of<R: Runtime>(app: &AppHandle<R>) -> Option<UpdateService> {
    app.try_state::<UpdateService>().map(|service| service.inner().clone())
}

/// The status as it is now. An app without a service (an older run of the tests) says it never looks.
pub fn current_status<R: Runtime>(app: &AppHandle<R>) -> UpdateStatus {
    service_of(app).map_or_else(
        || {
            Machine::new(
                UpdateMode::Off,
                Some(mode::NO_KEY_REASON.to_string()),
                env!("CARGO_PKG_VERSION"),
            )
            .status()
            .clone()
        },
        |service| service.status(),
    )
}

/// The service, for the commands that act on it.
pub fn service<R: Runtime>(app: &AppHandle<R>) -> Result<UpdateService, CommandError> {
    service_of(app).ok_or_else(|| CommandError::invalid("This version doesn't check for updates"))
}

/// The tray's "Check for updates…": starts a check the user asked for, unless an update is already found or on its way
/// (the window shows it). Returns at once; the check runs on a thread of its own.
pub fn check_from_tray<R: Runtime>(app: &AppHandle<R>) {
    let Some(service) = service_of(app) else { return };
    if matches!(
        service.status().state,
        UpdateState::Available | UpdateState::Downloading | UpdateState::Installing
    ) {
        return;
    }
    let started = std::thread::Builder::new()
        .name("rt-update-check".into())
        .spawn(move || {
            service.check(true);
        });
    if let Err(error) = started {
        eprintln!("could not start the update check: {error}");
    }
}

/// The settings changed: the schedule follows the answer to the question.
pub fn settings_changed<R: Runtime>(app: &AppHandle<R>, settings: &Settings) {
    if let Some(service) = service_of(app) {
        service.consent_changed(settings.check_for_updates);
    }
}

/// Stops the service's scheduler thread, when the app ends: no scheduled check starts while it winds down.
pub fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    if let Some(service) = service_of(app) {
        service.shutdown();
    }
}

#[cfg(test)]
pub mod fake {
    //! A source and a host that record what they are asked to do, for the tests of the service and the commands.

    use std::collections::VecDeque;
    use std::sync::mpsc::Receiver;

    use super::*;

    /// What happened, in order: `check`, `download`, `install`, `shutdown`, `restart`.
    pub type Log = Arc<Mutex<Vec<String>>>;

    pub fn entries(log: &Log) -> Vec<String> {
        lock(log).clone()
    }

    pub fn found(version: &str) -> Found {
        Found {
            version: version.into(),
            date: Some("2026-10-22".into()),
            notes: Some("Notes".into()),
        }
    }

    pub struct FakeSource {
        pub log: Log,
        /// The answers to the checks, in order; once they are used up, "nothing new".
        pub checks: Mutex<VecDeque<Result<Option<Found>, String>>>,
        pub download: Mutex<Result<(), String>>,
        pub install: Mutex<Result<(), String>>,
        /// How many progress reports the download makes, each of `1/progress_steps` more.
        pub progress_steps: u64,
        /// A check waits for this before it answers.
        pub gate: Mutex<Option<Receiver<()>>>,
    }

    impl FakeSource {
        pub fn new(log: &Log) -> Self {
            FakeSource {
                log: Arc::clone(log),
                checks: Mutex::new(VecDeque::new()),
                download: Mutex::new(Ok(())),
                install: Mutex::new(Ok(())),
                progress_steps: 4,
                gate: Mutex::new(None),
            }
        }

        pub fn answering(self, checks: Vec<Result<Option<Found>, String>>) -> Self {
            *lock(&self.checks) = checks.into();
            self
        }

        pub fn checks_made(&self) -> usize {
            entries(&self.log).iter().filter(|entry| *entry == "check").count()
        }
    }

    impl UpdateSource for FakeSource {
        fn check(&self) -> Result<Option<Found>, String> {
            lock(&self.log).push("check".into());
            let gate = lock(&self.gate).take();
            if let Some(gate) = gate {
                let _ = gate.recv();
            }
            lock(&self.checks).pop_front().unwrap_or(Ok(None))
        }

        fn download(&self, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<(), String> {
            lock(&self.log).push("download".into());
            for step in 1..=self.progress_steps {
                progress(step * 100, Some(self.progress_steps * 100));
            }
            lock(&self.download).clone()
        }

        fn install(&self) -> Result<(), String> {
            lock(&self.log).push("install".into());
            lock(&self.install).clone()
        }
    }

    /// Records the statuses it is shown, and shutting down and restarting.
    pub struct FakeHost {
        pub log: Log,
        pub published: Mutex<Vec<UpdateStatus>>,
    }

    impl FakeHost {
        pub fn new(log: &Log) -> Self {
            FakeHost {
                log: Arc::clone(log),
                published: Mutex::new(Vec::new()),
            }
        }

        pub fn states(&self) -> Vec<UpdateState> {
            lock(&self.published).iter().map(|status| status.state).collect()
        }
    }

    impl UpdateHost for FakeHost {
        fn publish(&self, status: &UpdateStatus) {
            lock(&self.published).push(status.clone());
        }

        fn shutdown_engine(&self) {
            lock(&self.log).push("shutdown".into());
        }

        fn restart(&self) {
            lock(&self.log).push("restart".into());
        }
    }

    /// Shows statuses like the app does (an event, the tray), and records shutting down and restarting, which it
    /// doesn't do.
    pub struct EventHost<R: Runtime> {
        app: AppHost<R>,
        log: Log,
    }

    impl<R: Runtime> EventHost<R> {
        pub fn new(app: AppHost<R>, log: &Log) -> Self {
            EventHost {
                app,
                log: Arc::clone(log),
            }
        }
    }

    impl<R: Runtime> UpdateHost for EventHost<R> {
        fn publish(&self, status: &UpdateStatus) {
            self.app.publish(status);
        }

        fn shutdown_engine(&self) {
            lock(&self.log).push("shutdown".into());
        }

        fn restart(&self) {
            lock(&self.log).push("restart".into());
        }
    }

    /// A clock that starts when it is made.
    pub fn clock() -> Arc<dyn Clock> {
        Arc::new(SystemClock::new())
    }

    /// Timings in which nothing is scheduled within a test.
    pub const NEVER: Timings = Timings {
        first_check: Duration::from_secs(3600),
        interval: Duration::from_secs(3600),
        retry: Duration::from_secs(3600),
        after_enabling: Duration::from_secs(3600),
    };

    pub fn install_mode() -> Detection {
        Detection {
            mode: UpdateMode::Install,
            reason: None,
        }
    }

    pub struct Fixture {
        pub service: UpdateService,
        pub source: Arc<FakeSource>,
        pub host: Arc<FakeHost>,
        pub log: Log,
    }

    pub fn fixture(
        detection: Detection,
        consent: Option<bool>,
        timings: Timings,
        shutdown: ShutdownTiming,
        configure: impl FnOnce(FakeSource) -> FakeSource,
    ) -> Fixture {
        let log: Log = Arc::default();
        let source = Arc::new(configure(FakeSource::new(&log)));
        let host = Arc::new(FakeHost::new(&log));
        let service = UpdateService::start(Parts {
            detection,
            current_version: "0.1.0".into(),
            consent,
            source: Arc::clone(&source) as Arc<dyn UpdateSource>,
            host: Arc::clone(&host) as Arc<dyn UpdateHost>,
            clock: clock(),
            timings,
            shutdown,
        });
        Fixture {
            service,
            source,
            host,
            log,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::fake::*;
    use super::*;
    use crate::ipc::CommandErrorKind;

    // The wire contract

    #[test]
    fn the_status_is_the_contract_with_the_window() {
        let status = UpdateStatus {
            mode: UpdateMode::Notify,
            mode_reason: Some("Install updates from the download page or your package manager.".into()),
            current_version: "0.1.0".into(),
            state: UpdateState::Available,
            available: Some(AvailableUpdate {
                version: "0.2.0".into(),
                date: Some("2026-10-22".into()),
                notes: "Notes".into(),
                release_url: "https://github.com/Roboroads/repeat-that/releases/tag/v0.2.0".into(),
            }),
            progress: Some(0.5),
            error: None,
            error_kind: None,
            last_checked_at: Some(1_700_000_000_000),
            manual: true,
        };
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            json!({
                "mode": "notify",
                "modeReason": "Install updates from the download page or your package manager.",
                "currentVersion": "0.1.0",
                "state": "available",
                "available": {
                    "version": "0.2.0",
                    "date": "2026-10-22",
                    "notes": "Notes",
                    "releaseUrl": "https://github.com/Roboroads/repeat-that/releases/tag/v0.2.0"
                },
                "progress": 0.5,
                "error": null,
                "errorKind": null,
                "lastCheckedAt": 1_700_000_000_000u64,
                "manual": true
            })
        );
    }

    #[test]
    fn modes_and_states_are_spelled_as_the_window_expects() {
        let modes = [
            UpdateMode::Install,
            UpdateMode::Notify,
            UpdateMode::External,
            UpdateMode::Off,
        ];
        assert_eq!(
            serde_json::to_value(modes).unwrap(),
            json!(["install", "notify", "external", "off"])
        );
        let states = [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::UpToDate,
            UpdateState::Available,
            UpdateState::Downloading,
            UpdateState::Installing,
            UpdateState::Error,
        ];
        assert_eq!(
            serde_json::to_value(states).unwrap(),
            json!([
                "idle",
                "checking",
                "upToDate",
                "available",
                "downloading",
                "installing",
                "error"
            ])
        );
    }

    #[test]
    fn the_failed_step_is_spelled_as_the_window_expects() {
        assert_eq!(
            serde_json::to_value([UpdateErrorKind::Check, UpdateErrorKind::Install]).unwrap(),
            json!(["check", "install"])
        );
    }

    #[test]
    fn the_event_is_called_update_status() {
        assert_eq!(STATUS_EVENT, "update-status");
    }

    // What is compiled in

    /// A public key file as `tauri signer generate` writes it: base64 of a comment line and the key line.
    fn sample_key() -> String {
        BASE64.encode("untrusted comment: minisign public key: 0123456789ABCDEF\nRWQBI0VniavN7wEjRWeJq83vASNFZ4mrze8BI0VniavN7w==\n")
    }

    #[test]
    fn a_public_key_file_is_taken_as_it_is() {
        let key = sample_key();
        assert_eq!(clean_pubkey(&key), Some(key.clone()));
    }

    #[test]
    fn the_newline_of_a_copied_key_file_is_dropped() {
        let key = sample_key();
        assert_eq!(clean_pubkey(&format!("{key}\n")), Some(key.clone()));
        assert_eq!(clean_pubkey(&format!("  {key}\r\n")), Some(key));
    }

    #[test]
    fn nothing_that_is_not_a_key_file_is_a_key() {
        let not_minisign = BASE64.encode("hello");
        for raw in [
            "",
            "   ",
            "\n",
            "not base64 at all!",
            not_minisign.as_str(),
            "dW50cnVzdGVk",
        ] {
            assert_eq!(clean_pubkey(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn a_plain_local_http_endpoint_is_local() {
        for url in [
            "http://127.0.0.1/latest.json",
            "http://127.0.0.1:8765/latest-linux-x86_64.json",
            "http://localhost:8765/x.json",
            "http://localhost/x.json",
            "http://127.0.0.1",
        ] {
            assert!(is_local_http(url), "{url}");
        }
    }

    #[test]
    fn nothing_else_counts_as_local_http() {
        for url in [
            "https://127.0.0.1/x.json",
            "http://example.com/x.json",
            "http://127.0.0.1.example.com/x.json",
            "http://localhost.example.com/x.json",
            "http://127.0.0.1:80@example.com/x.json",
            "http://127.0.0.1@example.com/x.json",
            "http://127.0.0.1:/x.json",
            "http://127.0.0.1:99999999/x.json",
            "http://127.0.0.1:8a/x.json",
            "http://[::1]/x.json",
            "http://0.0.0.0/x.json",
            "ftp://127.0.0.1/x.json",
            "127.0.0.1",
            "",
        ] {
            assert!(!is_local_http(url), "{url}");
        }
    }

    fn endpoint_of(config: &Value) -> &str {
        config["endpoints"][0].as_str().unwrap()
    }

    #[test]
    fn the_plugin_looks_for_the_release_file_of_its_own_platform() {
        let config = plugin_config("KEY", None);
        assert_eq!(config["pubkey"], "KEY");
        assert_eq!(config["dangerousInsecureTransportProtocol"], false);
        assert_eq!(
            endpoint_of(&config),
            "https://github.com/Roboroads/repeat-that/releases/latest/download/latest-{{target}}-{{arch}}.json"
        );
    }

    #[test]
    fn the_plugin_can_read_what_is_built() {
        let config = plugin_config(&sample_key(), None);
        let read: tauri_plugin_updater::Config = serde_json::from_value(config).expect("the plugin reads it");
        assert_eq!(read.endpoints.len(), 1);
        assert_eq!(read.pubkey, sample_key());
        assert!(!read.dangerous_insecure_transport_protocol);
        assert!(!read.dangerous_accept_invalid_certs);
        assert!(!read.dangerous_accept_invalid_hostnames);
        assert!(!read.allow_downgrades, "no downgrades");
    }

    #[test]
    fn a_local_endpoint_may_use_plain_http_and_only_then() {
        let config = plugin_config("KEY", Some("http://127.0.0.1:8765/latest-linux-x86_64.json"));
        assert_eq!(endpoint_of(&config), "http://127.0.0.1:8765/latest-linux-x86_64.json");
        assert_eq!(config["dangerousInsecureTransportProtocol"], true);
        let read: tauri_plugin_updater::Config = serde_json::from_value(config).unwrap();
        assert!(read.dangerous_insecure_transport_protocol);
    }

    #[test]
    fn another_https_endpoint_is_used_without_loosening_anything() {
        let config = plugin_config("KEY", Some(" https://updates.example.com/{{target}}.json "));
        assert_eq!(endpoint_of(&config), "https://updates.example.com/{{target}}.json");
        assert_eq!(config["dangerousInsecureTransportProtocol"], false);
    }

    #[test]
    fn an_endpoint_that_is_not_allowed_is_ignored() {
        for endpoint in [
            "http://example.com/x.json",
            "http://127.0.0.1:80@example.com/x.json",
            "ftp://example.com/x.json",
            "https://",
            "not a url",
        ] {
            let config = plugin_config("KEY", Some(endpoint));
            assert_eq!(endpoint_of(&config), ENDPOINT, "{endpoint}");
            assert_eq!(config["dangerousInsecureTransportProtocol"], false, "{endpoint}");
        }
        assert_eq!(endpoint_of(&plugin_config("KEY", Some("  "))), ENDPOINT);
    }

    #[test]
    fn this_build_has_no_updater_unless_a_key_was_set_when_it_was_built() {
        assert_eq!(is_compiled_in(), PUBKEY.and_then(clean_pubkey).is_some());
        assert_eq!(plugin::<tauri::Wry>().is_some(), is_compiled_in());
    }

    // Throttle and progress

    #[test]
    fn the_first_report_goes_out_and_then_one_per_interval() {
        let mut throttle = Throttle::new(Duration::from_millis(250));
        assert!(throttle.ready(Duration::from_millis(10)));
        assert!(!throttle.ready(Duration::from_millis(11)));
        assert!(!throttle.ready(Duration::from_millis(259)));
        assert!(throttle.ready(Duration::from_millis(260)));
        assert!(!throttle.ready(Duration::from_millis(300)));
        assert!(throttle.ready(Duration::from_millis(510)));
    }

    #[test]
    fn progress_is_the_part_that_arrived() {
        assert_eq!(fraction(0, Some(200)), Some(0.0));
        assert_eq!(fraction(50, Some(200)), Some(0.25));
        assert_eq!(fraction(200, Some(200)), Some(1.0));
    }

    #[test]
    fn progress_without_a_size_is_unknown() {
        assert_eq!(fraction(50, None), None);
        assert_eq!(fraction(50, Some(0)), None);
    }

    #[test]
    fn more_than_the_announced_size_is_still_at_most_the_whole() {
        assert_eq!(fraction(300, Some(200)), Some(1.0));
    }

    // The service: checks

    fn service(consent: Option<bool>, configure: impl FnOnce(FakeSource) -> FakeSource) -> Fixture {
        fixture(install_mode(), consent, NEVER, ShutdownTiming::AfterInstall, configure)
    }

    #[test]
    fn the_service_shows_its_first_status_at_once() {
        let f = service(None, |s| s);
        assert_eq!(f.host.states(), [UpdateState::Idle]);
        assert_eq!(f.service.status().state, UpdateState::Idle);
        assert_eq!(f.service.status().current_version, "0.1.0");
    }

    #[test]
    fn a_check_shows_checking_and_then_the_answer() {
        let f = service(None, |s| s.answering(vec![Ok(Some(found("0.2.0")))]));
        let status = f.service.check(true);
        assert_eq!(status.state, UpdateState::Available);
        assert!(status.manual);
        assert!(status.last_checked_at.is_some());
        assert_eq!(
            f.host.states(),
            [UpdateState::Idle, UpdateState::Checking, UpdateState::Available]
        );
        assert_eq!(f.service.status(), status);
    }

    #[test]
    fn a_failed_check_is_an_error_with_the_sentence_of_the_source() {
        let f = service(None, |s| s.answering(vec![Err("Couldn't check for updates.".into())]));
        let status = f.service.check(true);
        assert_eq!(status.state, UpdateState::Error);
        assert_eq!(status.error.as_deref(), Some("Couldn't check for updates."));
        assert_eq!(status.error_kind, Some(UpdateErrorKind::Check));
        assert_eq!(status.available, None);
    }

    #[test]
    fn a_check_during_a_check_gets_the_status_and_asks_nothing() {
        let (open, gate) = std::sync::mpsc::channel();
        let f = service(None, |s| {
            *s.gate.lock().unwrap() = Some(gate);
            s.answering(vec![Ok(Some(found("0.2.0")))])
        });
        let running = {
            let service = f.service.clone();
            std::thread::spawn(move || service.check(true))
        };
        let start = Instant::now();
        while f.service.status().state != UpdateState::Checking {
            assert!(start.elapsed() < Duration::from_secs(10), "the check starts");
            std::thread::sleep(Duration::from_millis(5));
        }
        let second = f.service.check(false);
        assert_eq!(second.state, UpdateState::Checking);
        assert!(second.manual, "the check that runs is the user's");
        open.send(()).unwrap();
        assert_eq!(running.join().unwrap().state, UpdateState::Available);
        assert_eq!(f.source.checks_made(), 1);
    }

    #[test]
    fn an_install_that_never_looks_never_asks() {
        for mode in [UpdateMode::Off, UpdateMode::External] {
            let detection = Detection {
                mode,
                reason: Some("Because.".into()),
            };
            let f = fixture(detection, Some(true), NEVER, ShutdownTiming::AfterInstall, |s| s);
            let status = f.service.check(true);
            assert_eq!(status.state, UpdateState::Idle, "{mode:?}");
            assert_eq!(status.mode_reason.as_deref(), Some("Because."));
            assert_eq!(f.source.checks_made(), 0);
        }
    }

    // The service: installs

    fn available(f: &Fixture) {
        assert_eq!(f.service.check(true).state, UpdateState::Available);
    }

    /// Waits until the install thread has done what it does.
    fn wait_for(f: &Fixture, what: &str, done: impl Fn(&UpdateStatus) -> bool) -> UpdateStatus {
        let start = Instant::now();
        loop {
            let status = f.service.status();
            if done(&status) {
                return status;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "{what}; stuck at {status:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn install_service(shutdown: ShutdownTiming, configure: impl FnOnce(FakeSource) -> FakeSource) -> Fixture {
        let f = fixture(install_mode(), None, NEVER, shutdown, |s| {
            configure(s.answering(vec![Ok(Some(found("0.2.0")))]))
        });
        available(&f);
        lock(&f.log).clear();
        f
    }

    fn after_the_restart(f: &Fixture) -> Vec<String> {
        let start = Instant::now();
        while !entries(&f.log).contains(&"restart".to_string()) {
            assert!(start.elapsed() < Duration::from_secs(10), "{:?}", entries(&f.log));
            std::thread::sleep(Duration::from_millis(5));
        }
        entries(&f.log)
    }

    #[test]
    fn where_files_are_replaced_while_running_the_install_comes_before_the_shutdown() {
        // macOS and an AppImage: a failed install leaves a working app behind.
        let f = install_service(ShutdownTiming::AfterInstall, |s| s);
        f.service.install().unwrap();
        assert_eq!(after_the_restart(&f), ["download", "install", "shutdown", "restart"]);
        assert_eq!(f.service.status().state, UpdateState::Installing);
    }

    #[test]
    fn where_the_installer_ends_the_app_the_shutdown_comes_before_the_install() {
        // Windows: the installer closes the app, so the engine has to be stopped first.
        let f = install_service(ShutdownTiming::BeforeInstall, |s| s);
        f.service.install().unwrap();
        assert_eq!(after_the_restart(&f), ["download", "shutdown", "install", "restart"]);
    }

    #[test]
    fn the_download_and_install_are_shown_in_order() {
        let f = install_service(ShutdownTiming::AfterInstall, |s| s);
        f.service.install().unwrap();
        after_the_restart(&f);
        let after_the_check: Vec<_> = f
            .host
            .states()
            .into_iter()
            .skip_while(|state| *state != UpdateState::Available)
            .skip(1)
            .collect();
        assert_eq!(
            after_the_check,
            [
                UpdateState::Downloading,
                UpdateState::Downloading,
                UpdateState::Installing
            ]
        );
        assert_eq!(f.service.status().available.map(|u| u.version), Some("0.2.0".into()));
    }

    #[test]
    fn a_failed_download_is_an_error_and_nothing_is_shut_down() {
        let f = install_service(ShutdownTiming::AfterInstall, |s| {
            *s.download.lock().unwrap() = Err("Couldn't install the update: the download failed.".into());
            s
        });
        f.service.install().unwrap();
        let status = wait_for(&f, "the error", |s| s.state == UpdateState::Error);
        assert_eq!(
            status.error.as_deref(),
            Some("Couldn't install the update: the download failed.")
        );
        assert!(status.available.is_some(), "the window links to the release page");
        assert_eq!(status.error_kind, Some(UpdateErrorKind::Install));
        assert_eq!(status.progress, None);
        assert_eq!(entries(&f.log), ["download"]);
    }

    #[test]
    fn an_install_that_fails_where_the_app_keeps_running_leaves_it_running() {
        let f = install_service(ShutdownTiming::AfterInstall, |s| {
            *s.install.lock().unwrap() = Err("Couldn't install the update.".into());
            s
        });
        f.service.install().unwrap();
        let status = wait_for(&f, "the error", |s| s.state == UpdateState::Error);
        assert_eq!(status.error.as_deref(), Some("Couldn't install the update."));
        assert!(status.available.is_some());
        assert_eq!(entries(&f.log), ["download", "install"], "no shutdown, no restart");
    }

    #[test]
    fn an_install_that_fails_after_the_engine_was_stopped_restarts_the_app() {
        let f = install_service(ShutdownTiming::BeforeInstall, |s| {
            *s.install.lock().unwrap() = Err("Couldn't install the update.".into());
            s
        });
        f.service.install().unwrap();
        assert_eq!(after_the_restart(&f), ["download", "shutdown", "install", "restart"]);
        assert_eq!(f.service.status().state, UpdateState::Error);
    }

    #[test]
    fn the_download_reports_progress_but_not_every_chunk() {
        let f = install_service(ShutdownTiming::AfterInstall, |mut s| {
            s.progress_steps = 500;
            s
        });
        f.service.install().unwrap();
        after_the_restart(&f);
        let downloading = f
            .host
            .states()
            .into_iter()
            .filter(|s| *s == UpdateState::Downloading)
            .count();
        // One for the start and a few as the progress comes in: 500 chunks within a few milliseconds are not 500 events.
        assert!((2..=4).contains(&downloading), "{downloading}");
    }

    #[test]
    fn progress_shows_how_much_has_arrived() {
        let f = install_service(ShutdownTiming::AfterInstall, |s| s);
        f.service.install().unwrap();
        after_the_restart(&f);
        let progress: Vec<_> = lock(&f.host.published)
            .iter()
            .filter(|s| s.state == UpdateState::Downloading)
            .map(|s| s.progress)
            .collect();
        assert_eq!(
            progress.first(),
            Some(&None),
            "the size isn't known when the download starts"
        );
        assert_eq!(progress.get(1), Some(&Some(0.25)), "the first report went out");
    }

    #[test]
    fn nothing_installs_without_an_update_or_twice() {
        let f = service(None, |s| s);
        let error = f.service.install().unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Invalid);
        assert!(entries(&f.log).is_empty());

        let f = install_service(ShutdownTiming::AfterInstall, |s| s);
        f.service.install().unwrap();
        after_the_restart(&f);
        assert_eq!(f.service.install().unwrap_err().kind, CommandErrorKind::Invalid);
        assert_eq!(entries(&f.log).iter().filter(|e| *e == "download").count(), 1);
    }

    #[test]
    fn an_install_that_only_announces_is_never_installed() {
        let detection = Detection {
            mode: UpdateMode::Notify,
            reason: Some("Install updates from the download page or your package manager.".into()),
        };
        let f = fixture(detection, None, NEVER, ShutdownTiming::AfterInstall, |s| {
            s.answering(vec![Ok(Some(found("0.2.0")))])
        });
        assert_eq!(f.service.check(true).state, UpdateState::Available);
        assert_eq!(f.service.install().unwrap_err().kind, CommandErrorKind::Invalid);
        assert_eq!(entries(&f.log), ["check"]);
    }

    /// What a `.deb` or `.rpm` install looks like to the updater: Linux, a key, and neither `APPIMAGE` nor a Flatpak.
    struct PackageEnv;

    impl crate::session::Env for PackageEnv {
        fn var(&self, _: &str) -> Option<String> {
            None
        }
    }

    struct NothingWritable;

    impl mode::InstallFacts for NothingWritable {
        fn current_exe(&self) -> Option<std::path::PathBuf> {
            Some("/usr/bin/repeat-that".into())
        }

        fn exists(&self, _: &std::path::Path) -> bool {
            false
        }

        fn can_replace(&self, _: &std::path::Path) -> bool {
            false
        }
    }

    #[test]
    fn a_deb_or_rpm_is_told_about_updates_and_never_downloads_or_installs_one() {
        // The release's manifest for Linux points at the AppImage, which a package can't be updated with (and the
        // plugin would try to run the package manager for it). So these installs only announce.
        let build = BuildFacts {
            has_key: true,
            debug_build: false,
            mocked: false,
            os: Os::Linux,
        };
        let detection = mode::detect(build, &PackageEnv, &NothingWritable);
        assert_eq!(detection.mode, UpdateMode::Notify);
        assert_eq!(detection.reason.as_deref(), Some(mode::PACKAGE_REASON));

        let f = fixture(detection, Some(true), NEVER, ShutdownTiming::AfterInstall, |s| {
            s.answering(vec![Ok(Some(found("0.2.0")))])
        });
        let status = f.service.check(true);
        assert_eq!(status.state, UpdateState::Available);
        assert_eq!(
            status.available.map(|update| update.release_url),
            Some("https://github.com/Roboroads/repeat-that/releases/tag/v0.2.0".into()),
            "the window's download page"
        );
        assert_eq!(f.service.install().unwrap_err().kind, CommandErrorKind::Invalid);
        assert_eq!(
            entries(&f.log),
            ["check"],
            "nothing downloaded, nothing installed, nothing stopped"
        );
        assert_eq!(f.service.status().state, UpdateState::Available);
    }

    // The service: on schedule

    fn quick(first_check_ms: u64) -> Timings {
        Timings {
            first_check: Duration::from_millis(first_check_ms),
            interval: Duration::from_secs(3600),
            retry: Duration::from_millis(60),
            after_enabling: Duration::from_millis(30),
        }
    }

    fn wait_for_checks(f: &Fixture, count: usize) {
        let start = Instant::now();
        while f.source.checks_made() < count {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "{count} checks; {:?}",
                entries(&f.log)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn once_the_user_said_yes_the_first_check_comes_by_itself() {
        let f = fixture(
            install_mode(),
            Some(true),
            quick(30),
            ShutdownTiming::AfterInstall,
            |s| s.answering(vec![Ok(Some(found("0.2.0")))]),
        );
        wait_for_checks(&f, 1);
        wait_for(&f, "the update", |s| s.state == UpdateState::Available);
        assert!(!f.service.status().manual, "nobody asked for it");
        f.service.shutdown();
    }

    #[test]
    fn without_a_yes_nothing_is_checked_by_itself() {
        for consent in [None, Some(false)] {
            let f = fixture(install_mode(), consent, quick(10), ShutdownTiming::AfterInstall, |s| s);
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(f.source.checks_made(), 0, "{consent:?}");
            f.service.shutdown();
        }
    }

    #[test]
    fn saying_yes_later_starts_the_checks() {
        let f = fixture(install_mode(), None, quick(10), ShutdownTiming::AfterInstall, |s| s);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(f.source.checks_made(), 0);
        f.service.consent_changed(Some(true));
        wait_for_checks(&f, 1);
        f.service.shutdown();
    }

    #[test]
    fn saying_no_before_the_check_is_due_cancels_it() {
        let f = fixture(
            install_mode(),
            Some(true),
            quick(150),
            ShutdownTiming::AfterInstall,
            |s| s,
        );
        f.service.consent_changed(Some(false));
        std::thread::sleep(Duration::from_millis(350));
        assert_eq!(f.source.checks_made(), 0);
        f.service.shutdown();
    }

    #[test]
    fn a_check_that_failed_is_tried_again_soon() {
        let f = fixture(
            install_mode(),
            Some(true),
            quick(10),
            ShutdownTiming::AfterInstall,
            |s| s.answering(vec![Err("offline".into()), Ok(None)]),
        );
        wait_for_checks(&f, 2);
        wait_for(&f, "up to date", |s| s.state == UpdateState::UpToDate);
        f.service.shutdown();
    }

    #[test]
    fn where_the_app_does_not_look_the_schedule_stays_quiet() {
        let detection = Detection {
            mode: UpdateMode::External,
            reason: Some("Updates come through Flathub.".into()),
        };
        let f = fixture(detection, Some(true), quick(10), ShutdownTiming::AfterInstall, |s| s);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(f.source.checks_made(), 0);
        f.service.consent_changed(Some(true));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(f.source.checks_made(), 0);
    }

    #[test]
    fn a_stopped_service_checks_no_more() {
        let f = fixture(
            install_mode(),
            Some(true),
            quick(150),
            ShutdownTiming::AfterInstall,
            |s| s,
        );
        f.service.shutdown();
        std::thread::sleep(Duration::from_millis(350));
        assert_eq!(f.source.checks_made(), 0);
    }
}
