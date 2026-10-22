//! Linux backend: PipeWire, for the app list, capture and ducking. See [`PipeWireSource`].

mod apps;
mod capture;
mod guard;
mod icon;
mod registry;
mod volume;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use registry::{Client, Command, Connection};

use crate::{
    AppAudioSource, AudioApp, CaptureError, CaptureHandle, DuckLevel, FrameSink, IconImage, PlatformOptions, Result,
    clamp_duck_level, duck_journal, ramp,
};

pub(crate) const HAS_BACKEND: bool = true;
pub(crate) const HAS_DUCK_GUARD: bool = true;
pub(crate) const EXE_PATHS_IGNORE_CASE: bool = false;

/// What capture says on a system without PipeWire, such as one with only PulseAudio.
const NO_PIPEWIRE: &str =
    "Capture needs PipeWire, and it isn't running. Systems with only PulseAudio aren't supported.";

/// What capture says while PipeWire isn't running yet, as when the app starts before it at login.
const NO_PIPEWIRE_YET: &str = "PipeWire isn't running yet. Capture starts as soon as it is.";

/// The PipeWire source. A duck journal in `options` also gives it the duck guard, which reads the journal to know what
/// to restore, except in a Flatpak, where a guard can't outlive the app.
pub(crate) fn source(options: PlatformOptions) -> Result<Box<dyn AppAudioSource>> {
    let source = PipeWireSource::new()?;
    Ok(Box::new(match options.duck_journal {
        Some(journal) => {
            let source = source.with_duck_journal(journal);
            if guard::sandboxed() {
                source
            } else {
                source.with_duck_guard()
            }
        }
        None => source,
    }))
}

pub(crate) fn icon(exe: &Path, size_px: u32) -> Option<IconImage> {
    icon::icon(exe, size_px)
}

/// The duck guard itself: waits for `parent` to exit, then restores the streams `journal` still lists. Streams whose
/// app isn't playing stay in the journal for the next start. Always `true`: this OS has a guard.
pub(crate) fn run_duck_guard(journal: &Path, parent: u32) -> bool {
    guard::wait_for_exit(parent);
    if duck_journal::read(journal).is_empty() {
        return true;
    }
    // Connecting with the journal restores what it lists, and writes back what is left.
    if let Some(remote) = socket(|name| std::env::var_os(name)) {
        let _ = Connection::open(remote, Some(journal.to_owned()), Arc::default());
    }
    true
}

/// Where the PipeWire socket is, the way libpipewire looks for it: `$PIPEWIRE_REMOTE` (a name or a path, `pipewire-0`
/// by default) in `$PIPEWIRE_RUNTIME_DIR`, `$XDG_RUNTIME_DIR` or `$USERPROFILE`. `None` without a runtime folder.
fn socket(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    let remote = set("PIPEWIRE_REMOTE").unwrap_or_else(|| "pipewire-0".into());
    if Path::new(&remote).is_absolute() {
        return Some(remote.into());
    }
    let dir = set("PIPEWIRE_RUNTIME_DIR")
        .or_else(|| set("XDG_RUNTIME_DIR"))
        .or_else(|| set("USERPROFILE"))?;
    Some(Path::new(&dir).join(remote))
}

/// Where a PulseAudio server's socket is: `native` in `$PULSE_RUNTIME_PATH`, else in `pulse` in `$XDG_RUNTIME_DIR`.
/// pipewire-pulse serves the same socket, but only once PipeWire runs.
fn pulse_socket(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    let dir = set("PULSE_RUNTIME_PATH")
        .map(PathBuf::from)
        .or_else(|| set("XDG_RUNTIME_DIR").map(|dir| Path::new(&dir).join("pulse")))?;
    Some(dir.join("native"))
}

/// The socket to connect to. Without it, [`CaptureError::Unsupported`] where PipeWire won't come for this session:
/// there is no runtime folder, or PulseAudio serves its `pulse` socket instead. Otherwise PipeWire may start after us,
/// as when a window manager's config starts both at login, and that is a [`CaptureError::Backend`] error the app tries
/// again, as is a socket that is there but doesn't answer yet.
fn remote(socket: Option<PathBuf>, pulse: Option<PathBuf>, exists: impl Fn(&Path) -> bool) -> Result<PathBuf> {
    match socket {
        Some(socket) if exists(&socket) => Ok(socket),
        Some(_) if !pulse.is_some_and(|pulse| exists(&pulse)) => Err(CaptureError::Backend(NO_PIPEWIRE_YET.into())),
        _ => Err(CaptureError::Unsupported(NO_PIPEWIRE.into())),
    }
}

/// The time between the steps of a fade: one cycle of the graph (`quantum`, in nanoseconds; 1024 frames at 48 kHz if
/// no capture measured it yet), so each step reaches the audio in a cycle of its own. Kept between 10 ms and 43 ms
/// (2048 frames), so a fade neither outruns the graph nor takes ages.
fn step_interval(quantum: u64) -> Duration {
    let quantum = if quantum == 0 {
        1024 * 1_000_000_000 / 48_000
    } else {
        quantum
    };
    Duration::from_nanos(quantum).clamp(Duration::from_millis(10), Duration::from_micros(42_700))
}

/// Captures one app's playback streams through PipeWire (0.3.65 or later), and ducks the app by lowering its streams'
/// volume.
///
/// The app list has one entry per app: its streams' process tree where `/proc` shows it, and otherwise its streams'
/// binary or name. Capture links the output ports of every stream of the app to a capture stream of our own, also
/// streams the app opens later; nothing else, such as a microphone, is ever linked. Streams apply their volume before
/// their output ports, so captures of a ducked app are amplified by the inverse of the duck level, and ducking fades
/// in steps of one graph cycle; see the `capture` and `ramp` modules.
///
/// The source talks to PipeWire from a thread of its own. If PipeWire restarts, running captures end and the next call
/// connects again. WirePlumber remembers each app's volume, so dropping the source restores every app it still ducks.
/// To survive a crash as well, see [`PipeWireSource::with_duck_journal`] and [`PipeWireSource::with_duck_guard`].
pub struct PipeWireSource {
    remote: PathBuf,
    journal: Option<PathBuf>,
    guard: Mutex<Guard>,
    connection: Mutex<Option<Connection>>,
    /// The length of the graph's cycle in nanoseconds, as captures last measured it.
    quantum: Arc<AtomicU64>,
    /// Current duck level of each app, shared with its captures.
    levels: Mutex<HashMap<String, DuckLevel>>,
    /// Held during a fade, with when its latest step started, so the next fade keeps its distance.
    fade: Mutex<Option<Instant>>,
}

#[derive(Debug, Default, PartialEq)]
enum Guard {
    #[default]
    Off,
    /// Starts on the first duck, so apps that never duck don't get an extra process.
    Wanted,
    /// Started, or failed to start. Either way only once: the journal still covers a failed start.
    Started,
}

impl PipeWireSource {
    /// The source for the PipeWire of this session. [`CaptureError::Unsupported`] if PipeWire won't run in it, as
    /// where PulseAudio runs instead, and a [`CaptureError::Backend`] error to try again while PipeWire isn't running
    /// yet. It connects on first use.
    pub fn new() -> Result<Self> {
        let var = |name: &str| std::env::var_os(name);
        let remote = remote(socket(var), pulse_socket(var), Path::exists)?;
        Ok(Self {
            remote,
            journal: None,
            guard: Mutex::default(),
            connection: Mutex::default(),
            quantum: Arc::default(),
            levels: Mutex::default(),
            fade: Mutex::default(),
        })
    }

    /// Keeps a journal of the ducked streams at `path`, and first restores the streams a journal left there by an
    /// earlier run lists, e.g. after a crash. Those of apps that aren't playing yet are restored once they play. Blocks
    /// while it connects.
    pub fn with_duck_journal(mut self, path: PathBuf) -> Self {
        self.journal = Some(path);
        // Restores what it can now; if PipeWire isn't up yet, the first call that connects does.
        let _ = self.client();
        self
    }

    /// Starts a [duck guard](crate::duck_guard) the first time an app is ducked, which restores ducked apps as soon as
    /// this process dies without doing so itself. Without it, they are restored at the next start, from the journal.
    ///
    /// Opt-in, because the guard is this executable (or the AppImage it runs from) started again: `main` must call
    /// [`duck_guard::run_if_requested`](crate::duck_guard::run_if_requested) first. Needs
    /// [`with_duck_journal`](Self::with_duck_journal), which tells the guard what to restore.
    pub fn with_duck_guard(self) -> Self {
        *lock(&self.guard) = Guard::Wanted;
        self
    }

    /// The connection, made now if there is none or it broke.
    fn client(&self) -> Result<Client> {
        let mut connection = lock(&self.connection);
        if let Some(live) = connection.as_ref().filter(|c| c.is_alive()) {
            return Ok(live.client());
        }
        // The old one, if any, ends first, and with it every duck: its captures are gone, and the next connection
        // ducks nothing but restores what the journal lists. The daemon may have restarted meanwhile, which gives
        // every stream the volume it remembers. A capture of an app that was ducked must not boost it now.
        *connection = None;
        let since = capture::now();
        for level in lock(&self.levels).values() {
            level.set(1.0, since);
        }
        let fresh = Connection::open(self.remote.clone(), self.journal.clone(), Arc::clone(&self.quantum))?;
        let client = fresh.client();
        *connection = Some(fresh);
        Ok(client)
    }

    /// The duck level of `app`, which its captures compensate for. One per app in the list: two instances of one
    /// executable are two apps with a process of their own, and ducking one doesn't make a capture of the other boost
    /// it. An app without a known process has one by its executable, or its name.
    fn level(&self, app: &AudioApp) -> DuckLevel {
        let key = if app.pid != 0 {
            format!("pid:{}", app.pid)
        } else if let Some(exe) = &app.exe_path {
            format!("exe:{}", exe.display())
        } else {
            format!("name:{}", app.name)
        };
        lock(&self.levels).entry(key).or_insert_with(DuckLevel::new).clone()
    }

    fn start_guard(&self) {
        let mut guard = lock(&self.guard);
        if *guard == Guard::Wanted {
            *guard = Guard::Started;
            if let Some(journal) = &self.journal {
                let _ = guard::spawn(journal);
            }
        }
    }
}

impl AppAudioSource for PipeWireSource {
    fn list_apps(&self) -> Result<Vec<AudioApp>> {
        self.client()?.request(Command::ListApps)
    }

    fn start(&self, app: &AudioApp, sink: FrameSink) -> Result<Box<dyn CaptureHandle>> {
        let client = self.client()?;
        let running = Arc::new(AtomicBool::new(true));
        let id = client.request(|reply| Command::Start {
            app: app.clone(),
            sink,
            level: self.level(app),
            running: Arc::clone(&running),
            reply,
        })?;
        Ok(Box::new(PipeWireCapture { id, running, client }))
    }

    fn set_app_volume(&self, app: &AudioApp, level: f32) -> Result<()> {
        let level = clamp_duck_level(level);
        let client = self.client()?;
        if level < 1.0 {
            self.start_guard();
        }
        let duck_level = self.level(app);
        // Held for the whole fade, so a second call can't interleave with it. The callers are off the UI thread.
        let mut last_step = lock(&self.fade);
        let interval = step_interval(self.quantum.load(Ordering::Relaxed));
        let mut result = Ok(());
        let mut first = true;
        ramp::run(duck_level.get(), level, interval, &mut last_step, |step, duck_new| {
            if result.is_ok() {
                result = client.request(|reply| Command::Fade {
                    app: app.clone(),
                    level: step,
                    duck_new,
                    first,
                    duck_level: duck_level.clone(),
                    reply,
                });
            }
            first = false;
        });
        result
    }
}

impl Drop for PipeWireSource {
    /// Restores every app that is still ducked. WirePlumber remembers per-app volumes, so it would stay quiet
    /// otherwise.
    fn drop(&mut self) {
        let connection = lock(&self.connection).take();
        if let Some(connection) = connection.filter(|c| c.is_alive()) {
            let _ = connection.client().request(Command::RestoreAll);
        }
        // Captures that outlive the source stop boosting.
        let since = capture::now() + capture::VOLUME_DELAY;
        for level in lock(&self.levels).values() {
            level.set(1.0, since);
        }
    }
}

/// A running capture; stopping or dropping it removes its stream and links.
struct PipeWireCapture {
    id: u64,
    running: Arc<AtomicBool>,
    client: Client,
}

impl CaptureHandle for PipeWireCapture {
    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed) && self.client.is_alive()
    }

    fn stop(self: Box<Self>) {
        // Drop does the work.
    }
}

impl Drop for PipeWireCapture {
    fn drop(&mut self) {
        let id = self.id;
        let _ = self.client.request(|reply| Command::Stop { id, reply });
    }
}

/// The file name of `path`, an executable or the program of an `Exec` line, or "" if it has none or it isn't UTF-8.
/// The app list and the `.desktop` lookup both name programs with it, so a program has the same name in both.
fn file_name(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or_default()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::net::UnixListener;

    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn the_socket_is_found_where_libpipewire_looks() {
        let socket = |vars: &[(&str, &str)]| socket(env(vars));
        assert_eq!(
            socket(&[("XDG_RUNTIME_DIR", "/run/user/1000")]),
            Some("/run/user/1000/pipewire-0".into())
        );
        assert_eq!(
            socket(&[
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
                ("PIPEWIRE_RUNTIME_DIR", "/run/pw")
            ]),
            Some("/run/pw/pipewire-0".into())
        );
        assert_eq!(
            socket(&[("XDG_RUNTIME_DIR", "/run/user/1000"), ("PIPEWIRE_REMOTE", "pipewire-1")]),
            Some("/run/user/1000/pipewire-1".into())
        );
        assert_eq!(
            socket(&[
                ("PIPEWIRE_REMOTE", "/tmp/pw/socket"),
                ("XDG_RUNTIME_DIR", "/run/user/1000")
            ]),
            Some("/tmp/pw/socket".into())
        );
        assert_eq!(
            socket(&[("XDG_RUNTIME_DIR", ""), ("USERPROFILE", "/home/me")]),
            Some("/home/me/pipewire-0".into())
        );
        assert_eq!(socket(&[]), None);
    }

    #[test]
    fn pulseaudio_alone_means_no_pipewire_for_the_session() {
        let socket = || Some(PathBuf::from("/run/user/1000/pipewire-0"));
        let pulse = || Some(PathBuf::from("/run/user/1000/pulse/native"));
        let only_pulse = remote(socket(), pulse(), |path| path.ends_with("native"));
        match only_pulse {
            Err(CaptureError::Unsupported(why)) => assert_eq!(why, NO_PIPEWIRE),
            other => panic!("expected Unsupported, got {other:?}"),
        }
        // Without a runtime folder there is no session to run PipeWire in.
        assert!(matches!(
            remote(None, None, |_| true),
            Err(CaptureError::Unsupported(_))
        ));
        assert_eq!(remote(Some("/s".into()), pulse(), |_| true).unwrap(), Path::new("/s"));
    }

    #[test]
    fn without_any_socket_yet_pipewire_may_still_start() {
        // A window manager's config that starts us before `exec pipewire`, say.
        let socket = Some(PathBuf::from("/run/user/1000/pipewire-0"));
        let pulse = Some(PathBuf::from("/run/user/1000/pulse/native"));
        match remote(socket, pulse, |_| false) {
            Err(CaptureError::Backend(why)) => assert_eq!(why, NO_PIPEWIRE_YET),
            other => panic!("expected a Backend error, got {other:?}"),
        }
    }

    #[test]
    fn the_pulse_socket_is_found_where_pulseaudio_puts_it() {
        let pulse = |vars: &[(&str, &str)]| pulse_socket(env(vars));
        assert_eq!(
            pulse(&[("XDG_RUNTIME_DIR", "/run/user/1000")]),
            Some("/run/user/1000/pulse/native".into())
        );
        assert_eq!(
            pulse(&[("XDG_RUNTIME_DIR", "/run/user/1000"), ("PULSE_RUNTIME_PATH", "/run/pa")]),
            Some("/run/pa/native".into())
        );
        assert_eq!(pulse(&[]), None);
    }

    #[test]
    fn a_socket_that_doesnt_answer_is_a_backend_error_to_try_again() {
        // A socket nobody accepts on, like PipeWire's while it starts, and a plain file in its place.
        let dir = std::env::temp_dir().join(format!("rt-pw-socket-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let listener = UnixListener::bind(dir.join("pipewire-0")).unwrap();
        drop(listener);
        fs::write(dir.join("pipewire-1"), "").unwrap();
        for name in ["pipewire-0", "pipewire-1"] {
            match Connection::open(dir.join(name), None, Arc::default()) {
                Err(CaptureError::Backend(why)) => assert!(why.contains("PipeWire"), "{why}"),
                Err(other) => panic!("expected a Backend error, got {other:?}"),
                Ok(_) => panic!("connected to {name}"),
            }
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A source for the PipeWire at `remote`, which it hasn't connected to yet.
    fn source_at(remote: PathBuf) -> PipeWireSource {
        PipeWireSource {
            remote,
            journal: None,
            guard: Mutex::default(),
            connection: Mutex::default(),
            quantum: Arc::default(),
            levels: Mutex::default(),
            fade: Mutex::default(),
        }
    }

    #[test]
    fn each_instance_of_an_app_has_a_duck_level_of_its_own() {
        let source = source_at("/nonexistent/pipewire-0".into());
        let firefox = |pid| AudioApp {
            pid,
            name: "Firefox".into(),
            exe_path: Some("/usr/lib/firefox/firefox".into()),
        };
        source.level(&firefox(100)).set(0.25, 1);
        assert_eq!(source.level(&firefox(100)).get(), 0.25);
        assert_eq!(
            source.level(&firefox(200)).get(),
            1.0,
            "the other instance isn't ducked"
        );
        // Without a process, the executable tells apps apart, else the name.
        source.level(&firefox(0)).set(0.5, 1);
        assert_eq!(source.level(&firefox(0)).get(), 0.5);
        let named = |name: &str| AudioApp {
            pid: 0,
            name: name.into(),
            exe_path: None,
        };
        source.level(&named("Spotify")).set(0.2, 1);
        assert_eq!(source.level(&named("Spotify")).get(), 0.2);
        assert_eq!(source.level(&named("Discord")).get(), 1.0);
    }

    #[test]
    fn a_new_connection_starts_with_every_app_at_full_volume() {
        // An app ducked through a connection that broke, a PipeWire restart say; the socket doesn't answer yet.
        let dir = std::env::temp_dir().join(format!("rt-pw-reconnect-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        drop(UnixListener::bind(dir.join("pipewire-0")).unwrap());
        let source = source_at(dir.join("pipewire-0"));
        let app = AudioApp {
            pid: 1234,
            name: "Discord".into(),
            exe_path: Some("/usr/share/discord/Discord".into()),
        };
        source.level(&app).set(0.25, capture::now());
        assert!(matches!(source.client(), Err(CaptureError::Backend(_))));
        assert_eq!(source.level(&app).get(), 1.0, "the next capture isn't boosted");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fades_step_once_per_graph_cycle() {
        assert_eq!(step_interval(0), Duration::from_nanos(21_333_333));
        assert_eq!(step_interval(10_666_666), Duration::from_nanos(10_666_666));
        // Small quanta (low-latency setups) and huge ones are kept within bounds.
        assert_eq!(step_interval(1_333_333), Duration::from_millis(10));
        assert_eq!(step_interval(170_666_666), Duration::from_micros(42_700));
    }

    #[test]
    fn this_os_has_a_backend_and_a_guard_and_case_sensitive_paths() {
        const { assert!(HAS_BACKEND && HAS_DUCK_GUARD && !EXE_PATHS_IGNORE_CASE) };
    }

    #[test]
    fn programs_are_named_by_their_file_name() {
        assert_eq!(file_name(Path::new("/usr/lib/firefox/firefox")), "firefox");
        assert_eq!(file_name(Path::new("firefox")), "firefox");
        // A trailing slash, as an `Exec` line could have it, names the same program.
        assert_eq!(file_name(Path::new("/opt/app/")), "app");
        assert_eq!(file_name(Path::new("/")), "");
        assert_eq!(file_name(Path::new("")), "");
    }
}
