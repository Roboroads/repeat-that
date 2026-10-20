//! Audio engine of Repeat That: wires an [`AppAudioSource`] (capture), rt-core's [`Recorder`](rt_core::Recorder) and
//! [`Player`](rt_core::Player) (speech buffer and replay) and an [`OutputBackend`] (playback) together on their own
//! threads, and ducks the captured app while a replay plays.
//!
//! ```text
//! capture thread --FrameSink--> rtrb queue --> worker thread: Recorder::push (speech buffer, behind a mutex)
//! player thread: Player::render 10 ms blocks --> resample to the device rate --> rtrb queue --> output callback
//! control thread: commands, reconnect poll every 2 s, capture start/stop
//! duck thread: the blocking set_app_volume fades, latest level wins
//! ```
//!
//! [`Engine::new`] starts the threads, [`Engine::handle`] gives a cloneable [`EngineHandle`] to control it, and
//! dropping the engine stops everything, wipes the buffer and gives the app its volume back.
//!
//! Captured audio never leaves memory, see `PRIVACY.md`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rt_capture::{AppAudioSource, AudioApp, CaptureError};
use rt_core::Vad;

mod beep;
mod control;
mod duck;
mod output;
mod playback;
mod status;
mod worker;

pub use output::{
    CpalOutput, NullOutput, OutputBackend, OutputDevice, OutputError, OutputFeed, OutputStream, TestOutput,
};
pub use status::{CaptureState, OutputStatus, PlaybackState, Status};

use control::ControlCmd;
use duck::DuckCmd;
use playback::PlayCmd;
use status::StatusHub;
use worker::{Core, VadSource, WorkerCmd};

/// How sure the voice activity detector must be before audio counts as speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sensitivity {
    /// Only clear speech; less background noise gets in.
    Low,
    #[default]
    Normal,
    /// Also quiet or distant speech.
    High,
}

impl Sensitivity {
    /// The segmenter's speech threshold for this sensitivity.
    pub fn threshold(self) -> f32 {
        match self {
            Self::Low => 0.65,
            Self::Normal => 0.5,
            Self::High => 0.35,
        }
    }
}

/// Settings the engine applies while it runs, see [`EngineHandle::set_config`].
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    /// How much speech to keep. Shrinking it evicts the oldest utterances at once.
    pub buffer: Duration,
    /// Applied from the next VAD frame on, without wiping the buffer.
    pub sensitivity: Sensitivity,
    /// Speed of a replay, `1.0` is normal.
    pub replay_speed: f32,
    /// Speed while catching up, see [`EngineHandle::toggle_catch_up`].
    pub catch_up_speed: f32,
    /// Level the captured app is ducked to while a replay plays, `None` to leave its volume alone. A change applies at
    /// the next replay.
    pub duck: Option<f32>,
    /// Id of the output device (see [`OutputDevice::id`]), `None` for the system default. A device that is missing or
    /// fails to open falls back to the default. A change applies at the next replay.
    pub output_device: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            buffer: Duration::from_secs(30),
            sensitivity: Sensitivity::Normal,
            replay_speed: 1.0,
            catch_up_speed: 1.5,
            duck: Some(0.2),
            output_device: None,
        }
    }
}

/// The app to capture, remembered across restarts of that app (its PID changes, its executable doesn't).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetApp {
    pub exe_path: Option<PathBuf>,
    pub name: String,
}

impl TargetApp {
    /// Whether `app` is this target: the same executable (case-insensitive on Windows), or the same name when the
    /// target has no executable (the mock source).
    pub fn matches(&self, app: &AudioApp) -> bool {
        match (&self.exe_path, &app.exe_path) {
            (Some(target), Some(exe)) => same_path(target, exe),
            (Some(_), None) => false,
            (None, _) => self.name == app.name,
        }
    }
}

impl From<&AudioApp> for TargetApp {
    fn from(app: &AudioApp) -> Self {
        Self {
            exe_path: app.exe_path.clone(),
            name: app.name.clone(),
        }
    }
}

#[cfg(windows)]
fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
}

#[cfg(not(windows))]
fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b
}

/// The running engine. Dropping it stops playback and capture, wipes the buffer and restores the app's volume.
pub struct Engine {
    handle: EngineHandle,
    threads: Vec<(JoinHandle<()>, Shutdown)>,
}

/// Tells one thread to finish.
enum Shutdown {
    Control(Sender<ControlCmd>),
    Playback(Sender<PlayCmd>),
    Worker(Sender<WorkerCmd>),
    Duck(Sender<DuckCmd>),
}

impl Shutdown {
    fn send(&self) {
        // A thread that already ended has nothing left to do, so a failed send is fine.
        match self {
            Self::Control(tx) => _ = tx.send(ControlCmd::Shutdown),
            Self::Playback(tx) => _ = tx.send(PlayCmd::Shutdown),
            Self::Worker(tx) => _ = tx.send(WorkerCmd::Shutdown),
            Self::Duck(tx) => _ = tx.send(DuckCmd::Shutdown),
        }
    }
}

impl Engine {
    /// Starts the engine with Silero VAD, or the energy VAD if Silero can't load ([`Status::vad_fallback`]). Capture
    /// starts once a target is set.
    pub fn new(source: Arc<dyn AppAudioSource>, output: Box<dyn OutputBackend>, config: EngineConfig) -> Self {
        Self::start(source, output, config, VadSource::Silero)
    }

    /// Starts the engine with the given voice activity detector, e.g. the energy VAD for synthetic test audio.
    pub fn with_vad(
        source: Arc<dyn AppAudioSource>,
        output: Box<dyn OutputBackend>,
        config: EngineConfig,
        vad: Box<dyn Vad>,
    ) -> Self {
        Self::start(source, output, config, VadSource::Given(vad))
    }

    fn start(
        source: Arc<dyn AppAudioSource>,
        output: Box<dyn OutputBackend>,
        config: EngineConfig,
        vad: VadSource,
    ) -> Self {
        let output: Arc<dyn OutputBackend> = Arc::from(output);
        let status = Arc::new(StatusHub::default());
        let core = Arc::new(Mutex::new(Core::default()));
        let dropped = Arc::new(AtomicU64::new(0));
        let (control_tx, control_rx) = mpsc::channel();
        let (play_tx, play_rx) = mpsc::channel();
        let (worker_tx, worker_rx) = mpsc::channel();
        let (duck_tx, duck_rx) = mpsc::channel();

        let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| {
            thread::Builder::new()
                .name(name.into())
                .spawn(f)
                .expect("can't start an engine thread")
        };
        let worker = worker::Worker::new(Arc::clone(&core), Arc::clone(&status), &config, vad);
        let worker = spawn("rt-worker", Box::new(move || worker.run(worker_rx)));
        let duck = duck::Duck::new(Arc::clone(&source));
        let duck = spawn("rt-duck", Box::new(move || duck.run(duck_rx)));
        let player = {
            let (core, output, duck_tx, status) = (
                Arc::clone(&core),
                Arc::clone(&output),
                duck_tx.clone(),
                Arc::clone(&status),
            );
            // Made on its own thread: the output streams it opens stay there.
            let run = move || playback::PlayerThread::new(core, output, duck_tx, status, config).run(play_rx);
            spawn("rt-player", Box::new(run))
        };
        let control = control::Control::new(
            Arc::clone(&source),
            Arc::clone(&core),
            worker_tx.clone(),
            play_tx.clone(),
            duck_tx.clone(),
            Arc::clone(&status),
            Arc::clone(&dropped),
        );
        let control = spawn("rt-control", Box::new(move || control.run(control_rx)));

        let handle = EngineHandle {
            control: control_tx.clone(),
            playback: play_tx.clone(),
            worker: worker_tx.clone(),
            source,
            output,
            status,
            dropped,
        };
        Self {
            handle,
            // Stopped in this order: playback first, so nothing plays while capture stops and the buffer is wiped, and
            // the duck thread last, after the last replay asked it to duck.
            threads: vec![
                (player, Shutdown::Playback(play_tx)),
                (control, Shutdown::Control(control_tx)),
                (worker, Shutdown::Worker(worker_tx)),
                (duck, Shutdown::Duck(duck_tx)),
            ],
        }
    }

    pub fn handle(&self) -> EngineHandle {
        self.handle.clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        for (thread, shutdown) in self.threads.drain(..) {
            shutdown.send();
            let _ = thread.join();
        }
    }
}

/// Controls a running [`Engine`]. Cheap to clone; calls after the engine was dropped do nothing.
///
/// Everything but [`list_apps`](Self::list_apps) and [`list_output_devices`](Self::list_output_devices) returns at
/// once. Those two talk to the OS and can block, so don't call them on a UI thread.
#[derive(Clone)]
pub struct EngineHandle {
    control: Sender<ControlCmd>,
    playback: Sender<PlayCmd>,
    worker: Sender<WorkerCmd>,
    source: Arc<dyn AppAudioSource>,
    output: Arc<dyn OutputBackend>,
    status: Arc<StatusHub>,
    dropped: Arc<AtomicU64>,
}

impl EngineHandle {
    /// Captures `target` from now on, reconnecting whenever it (re)appears. A different target wipes the buffer, stops
    /// a replay and restores the old app's volume. `None` stops capturing.
    pub fn set_target(&self, target: Option<TargetApp>) {
        let _ = self.control.send(ControlCmd::SetTarget(target));
    }

    /// Pausing stops capture, wipes the buffer, stops a replay and restores the app's volume.
    pub fn set_paused(&self, paused: bool) {
        let _ = self.control.send(ControlCmd::SetPaused(paused));
    }

    /// The replay button: replays the latest speech and plays on to live, or steps one utterance back during a replay.
    /// With nothing to replay, a short beep.
    pub fn replay(&self) {
        let _ = self.playback.send(PlayCmd::Replay);
    }

    /// Stops a replay.
    pub fn stop(&self) {
        let _ = self.playback.send(PlayCmd::Stop);
    }

    /// While idle, replays at catch-up speed. During a replay, switches between catch-up and replay speed at once.
    pub fn toggle_catch_up(&self) {
        let _ = self.playback.send(PlayCmd::ToggleCatchUp);
    }

    pub fn set_config(&self, config: EngineConfig) {
        let _ = self.worker.send(WorkerCmd::SetConfig(config.clone()));
        let _ = self.playback.send(PlayCmd::SetConfig(config));
    }

    /// Apps that can be captured right now. Blocks.
    pub fn list_apps(&self) -> Result<Vec<AudioApp>, CaptureError> {
        self.source.list_apps()
    }

    /// Output devices. Blocks.
    pub fn list_output_devices(&self) -> Result<Vec<OutputDevice>, OutputError> {
        self.output.devices()
    }

    /// The current status.
    pub fn status(&self) -> Status {
        self.status.get()
    }

    /// Receives the status on every change, and every 500 ms during a replay. The current status comes first. A
    /// receiver that falls 64 statuses behind is disconnected (its iterator ends); subscribe again to go on.
    pub fn subscribe(&self) -> Receiver<Status> {
        self.status.subscribe()
    }

    /// Captured samples dropped because the worker fell behind. Should stay 0.
    pub fn dropped_samples(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, exe: Option<&str>) -> AudioApp {
        AudioApp {
            pid: 1,
            name: name.into(),
            exe_path: exe.map(PathBuf::from),
        }
    }

    #[test]
    fn matches_by_executable_then_by_name() {
        let discord = TargetApp::from(&app("Discord", Some("/apps/Discord.exe")));
        assert!(discord.matches(&app("Discord (2)", Some("/apps/Discord.exe"))));
        assert!(!discord.matches(&app("Discord", Some("/apps/Other.exe"))));
        assert!(!discord.matches(&app("Discord", None)));

        let mock = TargetApp::from(&app("speech_8s", None));
        assert!(mock.matches(&app("speech_8s", None)));
        assert!(!mock.matches(&app("speech", None)));
    }

    #[test]
    #[cfg(windows)]
    fn matches_executables_case_insensitively_on_windows() {
        let target = TargetApp::from(&app("Discord", Some(r"C:\Apps\Discord.exe")));
        assert!(target.matches(&app("Discord", Some(r"c:\apps\discord.EXE"))));
    }

    #[test]
    fn engine_and_handle_can_live_in_app_state() {
        fn shared<T: Send + Sync + 'static>() {}
        shared::<Engine>();
        shared::<EngineHandle>();
    }

    #[test]
    fn sensitivity_thresholds() {
        assert_eq!(Sensitivity::default(), Sensitivity::Normal);
        assert!(Sensitivity::Low.threshold() > Sensitivity::Normal.threshold());
        assert!(Sensitivity::High.threshold() < Sensitivity::Normal.threshold());
    }
}
