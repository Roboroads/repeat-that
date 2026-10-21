//! End-to-end tests of the macOS backend against a real audio device: `afplay` plays a tone fixture, and we list,
//! capture and duck it, and check that a process killed while it ducks leaves the app audible. CI (the macOS capture
//! workflow) installs BlackHole 2ch as the output device, so what the device plays can be recorded from its input, and
//! tries to give the runner the audio capture and microphone permissions. Run them locally with
//!
//! ```sh
//! cargo build -p rt-capture --example probe
//! cargo test -p rt-capture --test macos_audio -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `RT_REQUIRE_TCC=1` says the permissions were given, and makes a missing one fail the tests instead of skipping them.
//! The tone is synthetic, `afplay` only reads it, and everything captured or recorded stays in memory. Lines starting
//! with `summary:` go to the workflow's step summary.

#![cfg(target_os = "macos")]

use std::process::{Child, Command};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use std::{env, thread};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rt_capture::macos::{CoreAudioSource, responsible_pid};
use rt_capture::permission::{self, PermissionStatus};
use rt_capture::{AppAudioSource, AudioApp, CaptureError, CaptureFormat, CaptureHandle, FrameSink};

const TONE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tone_1khz.wav");
/// Noise for the click check: a late copy of it adds to it instead of cancelling it, as a copy of the tone can.
const NOISE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/noise.wav");
/// The tone is a sine at half of full scale.
const TONE_RMS: f64 = 0.3536;
/// How far a level may be from what it should be, as a ratio.
const LEVEL_TOLERANCE: f64 = 0.15;

/// The tests play on the same device, so they take turns.
static AUDIO: Mutex<()> = Mutex::new(());

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A child process that is killed when the test ends, also on a failed assertion.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Plays the tone fixture.
fn play_tone() -> Killed {
    play(TONE)
}

fn play(fixture: &str) -> Killed {
    Killed(
        Command::new("/usr/bin/afplay")
            .arg(fixture)
            .spawn()
            .expect("start afplay"),
    )
}

/// Whether these tests may capture. With `RT_REQUIRE_TCC=1` they must, otherwise they are skipped without permission.
fn allowed() -> bool {
    let status = permission::status();
    if env::var_os("RT_REQUIRE_TCC").is_some_and(|v| v == "1") {
        assert!(
            matches!(status, Ok(PermissionStatus::Allowed)),
            "RT_REQUIRE_TCC=1, but capture isn't allowed: {status:?}"
        );
        return true;
    }
    let allowed = matches!(status, Ok(PermissionStatus::Allowed));
    if !allowed {
        println!("skipped: capture isn't allowed here ({status:?})");
    }
    allowed
}

fn wait_for<T>(timeout: Duration, mut found: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(value) = found() {
            return Some(value);
        }
        thread::sleep(Duration::from_millis(50));
    }
    None
}

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / samples.len() as f64).sqrt()
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.max(1e-9).log10()
}

/// The last `duration` of interleaved `samples`.
fn tail(samples: &[f32], channels: usize, rate: u32, duration: Duration) -> &[f32] {
    let len = (duration.as_secs_f64() * f64::from(rate)) as usize * channels;
    &samples[samples.len().saturating_sub(len)..]
}

/// A capture whose audio is kept in memory.
struct Recording {
    handle: Box<dyn CaptureHandle>,
    samples: Arc<Mutex<(Vec<f32>, Option<CaptureFormat>)>>,
}

impl Recording {
    /// Starts capturing `app` once it plays: a player that was just started takes a moment to open Core Audio.
    fn start(source: &dyn AppAudioSource, app: &AudioApp) -> Self {
        let samples = Arc::new(Mutex::new((Vec::new(), None)));
        let started = Instant::now();
        loop {
            let sink = Arc::clone(&samples);
            let sink: FrameSink = Box::new(move |frames, format| {
                let mut sink = lock(&sink);
                sink.0.extend_from_slice(frames);
                sink.1 = Some(format);
            });
            match source.start(app, sink) {
                Ok(handle) => return Self { handle, samples },
                Err(CaptureError::AppNotFound(_)) if started.elapsed() < Duration::from_secs(10) => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => panic!("capture {app:?}: {e}"),
            }
        }
    }

    /// What was captured since the last take.
    fn take(&self) -> (Vec<f32>, CaptureFormat) {
        let mut samples = lock(&self.samples);
        let format = samples.1.expect("the capture delivered audio");
        (std::mem::take(&mut samples.0), format)
    }

    /// What is captured over the next `duration`.
    fn record(&self, duration: Duration) -> (Vec<f32>, CaptureFormat) {
        lock(&self.samples).0.clear();
        thread::sleep(duration);
        self.take()
    }
}

/// What the virtual device plays, recorded from its input: BlackHole plays its output back on its input.
struct Loopback {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    channels: usize,
}

impl Loopback {
    fn open() -> Self {
        let host = cpal::default_host();
        let device = host
            .input_devices()
            .expect("list input devices")
            .find(|d| d.description().is_ok_and(|d| d.name().contains("BlackHole")))
            .expect("BlackHole is installed (see .github/scripts/macos-ci-audio.sh)");
        let supported = device.default_input_config().expect("BlackHole's input format");
        let config = supported.config();
        assert_eq!(
            supported.sample_format(),
            cpal::SampleFormat::F32,
            "BlackHole records f32"
        );
        let samples = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&samples);
        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| lock(&sink).extend_from_slice(data),
                |e| eprintln!("loopback: {e}"),
                None,
            )
            .expect("record BlackHole");
        stream.play().expect("start recording BlackHole");
        Self {
            _stream: stream,
            samples,
            rate: config.sample_rate,
            channels: usize::from(config.channels),
        }
    }

    fn take(&self) -> Vec<f32> {
        std::mem::take(&mut *lock(&self.samples))
    }

    fn record(&self, duration: Duration) -> Vec<f32> {
        self.take();
        thread::sleep(duration);
        self.take()
    }

    fn level(&self, duration: Duration) -> f64 {
        rms(&self.record(duration))
    }
}

/// A process to capture: the player itself, as an app made from its PID.
fn player_app(player: &Killed) -> AudioApp {
    AudioApp {
        pid: player.0.id(),
        name: "afplay".into(),
        exe_path: None,
    }
}

#[test]
#[ignore = "plays audio; needs BlackHole and the audio capture permission"]
fn captures_a_tone_at_its_level() {
    let _audio = lock(&AUDIO);
    if !allowed() {
        return;
    }
    let source = CoreAudioSource::new();
    let player = play_tone();
    let pid = player.0.id();

    // Listed under the process macOS holds responsible for it (the CI agent or a terminal), or as itself.
    let group = responsible_pid(pid).unwrap_or(pid);
    let listed = wait_for(Duration::from_secs(10), || {
        source
            .list_apps()
            .ok()?
            .into_iter()
            .find(|app| app.pid == group || app.pid == pid)
    })
    .expect("the player's app is listed");
    println!("listed: {listed:?} (player PID {pid})");

    let recording = Recording::start(&source, &player_app(&player));
    thread::sleep(Duration::from_millis(500));
    let (samples, format) = recording.record(Duration::from_secs(1));
    let level = rms(&samples);
    println!(
        "summary: captured the tone at {level:.3} RMS ({:+.2} dB of the file), {} Hz, {} channels, {:.2} s",
        db(level / TONE_RMS),
        format.sample_rate,
        format.channels,
        samples.len() as f64 / f64::from(format.channels) / f64::from(format.sample_rate)
    );
    assert_eq!(
        format,
        CaptureFormat {
            sample_rate: 48_000,
            channels: 2
        }
    );
    assert!(
        (level / TONE_RMS - 1.0).abs() < LEVEL_TOLERANCE,
        "captured at {level:.3}"
    );
    assert!(recording.handle.is_running());
}

/// The largest step between neighbouring samples of the first channel.
fn largest_step(samples: &[f32], channels: usize) -> f32 {
    let first: Vec<f32> = samples.chunks_exact(channels).map(|f| f[0]).collect();
    first.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
}

/// How a recording of the device moves between two levels, in windows of 5 ms relative to full level.
struct Transition {
    levels: Vec<f64>,
    quietest: f64,
    loudest: f64,
    /// The longest stretch below half of the lower level.
    gap_ms: f64,
    /// The longest stretch louder than both levels (+2.3 dB over the higher one, above the noise's own spread).
    bump_ms: f64,
    /// The largest step between neighbouring samples, relative to the largest at full level.
    step: f64,
}

const WINDOW_MS: usize = 5;

fn transition(samples: &[f32], loopback: &Loopback, full: f64, full_step: f32, levels: (f64, f64)) -> Transition {
    let window = loopback.rate as usize * WINDOW_MS / 1000 * loopback.channels;
    let windows: Vec<f64> = samples.chunks_exact(window).map(|w| rms(w) / full).collect();
    let longest = |inside: &dyn Fn(f64) -> bool| {
        let (mut longest, mut run) = (0, 0);
        for &level in &windows {
            run = if inside(level) { run + 1 } else { 0 };
            longest = longest.max(run);
        }
        (longest * WINDOW_MS) as f64
    };
    let (lower, higher) = (levels.0.min(levels.1), levels.0.max(levels.1));
    Transition {
        quietest: windows.iter().copied().fold(f64::INFINITY, f64::min),
        loudest: windows.iter().copied().fold(0.0, f64::max),
        gap_ms: longest(&|level| level < lower * 0.5),
        bump_ms: longest(&|level| level > higher * 1.3),
        step: f64::from(largest_step(samples, loopback.channels) / full_step),
        levels: windows,
    }
}

/// Prints the figures of the click check, and the levels the transition went through.
fn report(what: &str, t: &Transition) {
    let first: Vec<String> = t.levels.iter().take(80).map(|l| format!("{:.0}", l * 100.0)).collect();
    println!("{what}: levels in % of full, every {WINDOW_MS} ms: {}", first.join(" "));
    println!(
        "summary: {what}: quietest {WINDOW_MS} ms at {:.2}x, loudest {:.2}x ({:+.1} dB), a gap of {:.0} ms, louder than \
         both for {:.0} ms, largest step {:.2}x the noise's own",
        t.quietest,
        t.loudest,
        db(t.loudest),
        t.gap_ms,
        t.bump_ms,
        t.step
    );
}

/// The click check on a recording of the device: no gap, a bump of at most +6 dB (plus a little for the noise) that
/// doesn't last long, and no step between samples much larger than the noise makes by itself.
fn assert_smooth(what: &str, t: &Transition) {
    assert_eq!(t.gap_ms, 0.0, "{what}: a gap");
    assert!(db(t.loudest) <= 7.0, "{what}: a bump of {:+.1} dB", db(t.loudest));
    assert!(t.bump_ms <= 100.0, "{what}: louder for {} ms", t.bump_ms);
    assert!(t.step <= 2.5, "{what}: a step {:.2}x the noise's own", t.step);
}

#[test]
#[ignore = "plays audio; needs BlackHole and the audio capture and microphone permissions"]
fn ducking_replays_the_app_at_the_level_while_capture_keeps_full_level() {
    let _audio = lock(&AUDIO);
    if !allowed() {
        return;
    }
    let source = CoreAudioSource::new();
    let player = play(NOISE);
    let app = player_app(&player);
    let recording = Recording::start(&source, &app);
    let loopback = Loopback::open();
    thread::sleep(Duration::from_millis(500));
    let reference = loopback.record(Duration::from_secs(1));
    let (device_full, full_step) = (rms(&reference), largest_step(&reference, loopback.channels));
    let tap_full = rms(&recording.record(Duration::from_millis(500)).0);
    println!("summary: device at {device_full:.3} RMS, capture at {tap_full:.3} RMS before ducking");
    assert!(device_full > 0.1, "BlackHole plays the noise back ({device_full:.3})");

    loopback.take();
    recording.take();
    let started = Instant::now();
    source.set_app_volume(&app, 0.2).expect("duck to 20%");
    println!("summary: ducking to 20% took {} ms", started.elapsed().as_millis());
    thread::sleep(Duration::from_secs(1));
    let engage = loopback.take();
    let (tap, format) = recording.take();
    let channels = usize::from(format.channels);
    let device_ducked = rms(tail(
        &engage,
        loopback.channels,
        loopback.rate,
        Duration::from_millis(500),
    ));
    let tap_ducked = rms(tail(&tap, channels, format.sample_rate, Duration::from_millis(500)));
    println!(
        "summary: ducked to 20%: device at {:.2}x ({:+.1} dB), capture at {:.2}x",
        device_ducked / device_full,
        db(device_ducked / device_full),
        tap_ducked / tap_full
    );
    assert!(
        (device_ducked / device_full / 0.2 - 1.0).abs() < 0.25,
        "the device plays at 20%"
    );
    assert!(
        (tap_ducked / tap_full - 1.0).abs() < LEVEL_TOLERANCE,
        "capture keeps its level"
    );
    // Without a dropout while the app is muted.
    let window = format.sample_rate as usize * WINDOW_MS / 1000 * channels;
    let captured: Vec<f64> = tap.chunks_exact(window).map(|w| rms(w) / tap_full).collect();
    let quietest = captured.iter().copied().fold(f64::INFINITY, f64::min);
    let first: Vec<String> = captured.iter().take(80).map(|l| format!("{:.0}", l * 100.0)).collect();
    println!(
        "capture: levels in % of full, every {WINDOW_MS} ms: {}",
        first.join(" ")
    );
    println!("summary: capture while ducking: quietest {WINDOW_MS} ms at {quietest:.2}x");
    let engaged = transition(&engage, &loopback, device_full, full_step, (1.0, 0.2));

    let started = Instant::now();
    source.set_app_volume(&app, 1.0).expect("restore");
    println!("summary: restoring took {} ms", started.elapsed().as_millis());
    thread::sleep(Duration::from_secs(1));
    let restore = loopback.take();
    let device_restored = rms(tail(
        &restore,
        loopback.channels,
        loopback.rate,
        Duration::from_millis(500),
    ));
    println!("summary: restored: device at {:.2}x", device_restored / device_full);
    let restored = transition(&restore, &loopback, device_full, full_step, (0.2, 1.0));
    report("engage", &engaged);
    report("restore", &restored);
    assert_smooth("engage", &engaged);
    assert_smooth("restore", &restored);
    assert!(quietest >= 0.5, "the capture drops out while ducking ({quietest:.2}x)");
    assert!((device_restored / device_full - 1.0).abs() < LEVEL_TOLERANCE);
    assert!(recording.handle.is_running());
}

#[test]
#[ignore = "plays audio; needs BlackHole and the audio capture and microphone permissions"]
fn ducking_to_zero_mutes_the_device_while_capture_keeps_full_level() {
    let _audio = lock(&AUDIO);
    if !allowed() {
        return;
    }
    let source = CoreAudioSource::new();
    let player = play_tone();
    let app = player_app(&player);
    let recording = Recording::start(&source, &app);
    let loopback = Loopback::open();
    thread::sleep(Duration::from_millis(500));
    let device_full = loopback.level(Duration::from_secs(1));
    let tap_full = rms(&recording.record(Duration::from_millis(500)).0);

    source.set_app_volume(&app, 0.0).expect("mute");
    thread::sleep(Duration::from_millis(300));
    let device_muted = loopback.level(Duration::from_millis(500));
    let tap_muted = rms(&recording.record(Duration::from_millis(500)).0);
    println!(
        "summary: ducked to 0%: device at {:.3}x ({:+.1} dB), capture at {:.2}x",
        device_muted / device_full,
        db(device_muted / device_full),
        tap_muted / tap_full
    );
    assert!(device_muted / device_full < 0.02, "the device is silent");
    assert!(
        (tap_muted / tap_full - 1.0).abs() < LEVEL_TOLERANCE,
        "capture keeps its level"
    );

    source.set_app_volume(&app, 1.0).expect("unmute");
    thread::sleep(Duration::from_millis(300));
    let device_restored = loopback.level(Duration::from_millis(500));
    assert!(
        (device_restored / device_full - 1.0).abs() < LEVEL_TOLERANCE,
        "{device_restored:.3}"
    );
}

#[test]
#[ignore = "plays audio; needs BlackHole, the permissions and the probe example"]
fn a_killed_process_that_ducked_leaves_the_app_audible() {
    let _audio = lock(&AUDIO);
    if !allowed() {
        return;
    }
    let probe = env::current_exe()
        .ok()
        .and_then(|test| Some(test.parent()?.parent()?.join("examples/probe")))
        .unwrap_or_default();
    assert!(
        probe.exists(),
        "{} is missing; build it with `cargo build -p rt-capture --example probe`",
        probe.display()
    );
    let player = play_tone();
    let loopback = Loopback::open();
    thread::sleep(Duration::from_millis(500));
    let full = loopback.level(Duration::from_millis(500));

    // The probe holds the duck for a minute, far longer than the test needs.
    let prober = Killed(
        Command::new(&probe)
            .args(["duck", &player.0.id().to_string(), "0.2", "60"])
            .spawn()
            .expect("start the probe"),
    );
    wait_for(Duration::from_secs(15), || {
        let level = loopback.level(Duration::from_millis(100)) / full;
        (level < 0.3).then_some(())
    })
    .expect("the probe ducks the player");

    // Killed without a chance to restore anything, like a crash. macOS removes its private tap, and the mute with it.
    let pid = prober.0.id().to_string();
    let killed = Command::new("/bin/kill").args(["-9", &pid]).status().expect("run kill");
    assert!(killed.success());
    let start = Instant::now();
    let back = wait_for(Duration::from_secs(5), || {
        let level = loopback.level(Duration::from_millis(50)) / full;
        (level > 0.8).then(|| start.elapsed())
    });
    println!("summary: after kill -9 of the ducking process, full level again after {back:?}");
    let back = back.expect("the player is audible again");
    assert!(back <= Duration::from_secs(1), "audible again only after {back:?}");
    drop(prober);
}

#[test]
#[ignore = "plays audio; needs BlackHole and the audio capture permission"]
fn a_process_that_joins_the_app_is_captured_too() {
    let _audio = lock(&AUDIO);
    if !allowed() {
        return;
    }
    let source = CoreAudioSource::new();
    let first = play_tone();
    let Some(group) = responsible_pid(first.0.id()) else {
        println!("skipped: macOS doesn't say which process is responsible for the player");
        return;
    };
    let app = wait_for(Duration::from_secs(10), || {
        source.list_apps().ok()?.into_iter().find(|app| app.pid == group)
    })
    .expect("the player's app is listed");
    let recording = Recording::start(&source, &app);
    thread::sleep(Duration::from_millis(500));
    let first_level = rms(&recording.record(Duration::from_millis(500)).0);
    assert!(
        (first_level / TONE_RMS - 1.0).abs() < LEVEL_TOLERANCE,
        "the first player at {first_level:.3}"
    );

    // A second player joins the app, and the first one quits: what is captured then comes from the second one alone.
    let second = play_tone();
    // The watcher looks every 500 ms, or sooner when Core Audio says the process list changed.
    thread::sleep(Duration::from_secs(2));
    drop(first);
    thread::sleep(Duration::from_millis(500));
    let second_level = rms(&recording.record(Duration::from_millis(500)).0);
    println!(
        "summary: a second player in {} (PID {}) was captured at {second_level:.3} RMS after the first one quit",
        app.name,
        second.0.id()
    );
    assert!(
        (second_level / TONE_RMS - 1.0).abs() < LEVEL_TOLERANCE,
        "the second player at {second_level:.3}"
    );
    assert!(recording.handle.is_running());
}

#[test]
#[ignore = "plays audio; checks the error without the audio capture permission"]
fn without_permission_starting_says_so() {
    let _audio = lock(&AUDIO);
    let status = permission::status();
    println!("summary: permission status: {status:?}");
    if matches!(status, Ok(PermissionStatus::Allowed)) {
        println!("skipped: capture is allowed here");
        return;
    }
    let source = CoreAudioSource::new();
    let player = play_tone();
    thread::sleep(Duration::from_millis(500));
    match source.start(&player_app(&player), Box::new(|_, _| {})) {
        Err(CaptureError::PermissionDenied(how)) => println!("summary: without permission: denied, {how}"),
        Err(CaptureError::Backend(why)) if why.contains("Waiting for you to allow") => {
            println!("summary: without permission: {why}")
        }
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("capture started without permission"),
    }
}
