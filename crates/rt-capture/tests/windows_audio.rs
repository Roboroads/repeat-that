//! End-to-end tests of the Windows backend against a real audio device: a child process plays a tone, and we list,
//! capture and duck it, and check that the duck guard restores it when the process that ducked it is killed. CI
//! installs a virtual audio device for this. The last test reads an executable's icon from the shell and needs no
//! audio. Run them locally with
//!
//! ```sh
//! cargo build -p rt-capture --example probe
//! cargo test -p rt-capture --test windows_audio -- --ignored --nocapture
//! ```
//!
//! The tone is synthetic and everything stays in memory. The probe's duck journal holds volumes only.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};
use std::{env, thread};

use rt_capture::{AppAudioSource, AudioApp, CaptureFormat, WasapiSource, app_icon};

const TONE_HZ: f64 = 1000.0;
/// The player's sine has an amplitude of 0.5, so an RMS of about 0.35 at full volume.
const TONE_RMS: f64 = 0.354;
/// How far the compensated capture of a ducked app may stray from its level, in any 1 ms window. A fade step is at
/// most 0.5 dB and 10 ms long. On CI the steps of a fade land up to 10 ms before or 8 ms after the estimate, and now
/// and then a single step lands another 10 ms off, which leaves a few windows three steps (1.44 dB) off. The limit
/// allows four.
const MAX_OFF_DB: f64 = 2.0;
/// How many times the test fades down and back up: every fade lands at its own offset from the estimate.
const FADES: usize = 5;

/// Plays a looping 1 kHz tone through `System.Media.SoundPlayer`, built in memory.
const PLAYER: &str = "\
$rate = 48000; $n = $rate;
$ms = New-Object IO.MemoryStream; $w = New-Object IO.BinaryWriter($ms);
$w.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $w.Write([int](36 + $n * 2));
$w.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt ')); $w.Write([int]16);
$w.Write([int16]1); $w.Write([int16]1); $w.Write([int]$rate); $w.Write([int]($rate * 2));
$w.Write([int16]2); $w.Write([int16]16);
$w.Write([Text.Encoding]::ASCII.GetBytes('data')); $w.Write([int]($n * 2));
for ($i = 0; $i -lt $n; $i++) { $w.Write([int16](16384 * [Math]::Sin(2 * [Math]::PI * 1000 * $i / $rate))) }
$ms.Position = 0;
(New-Object Media.SoundPlayer($ms)).PlayLooping();
Start-Sleep 300";

/// The tests play on the same device and capture their own process tree, so they take turns.
static AUDIO: Mutex<()> = Mutex::new(());

/// A child process that is killed when the test ends, also on a failed assertion.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_player() -> Killed {
    Killed(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", PLAYER])
            .spawn()
            .expect("start PowerShell"),
    )
}

#[test]
#[ignore = "plays audio; needs an audio output device"]
fn lists_captures_and_ducks_a_playing_process() {
    let _audio = AUDIO.lock().unwrap_or_else(|e| e.into_inner());
    let source = WasapiSource::new();
    let player = start_player();
    let player_pid = player.0.id();

    // The app list shows the player once it has an audio session.
    let app = wait_for(Duration::from_secs(30), || {
        source
            .list_apps()
            .unwrap()
            .into_iter()
            .find(|app| app.pid == player_pid)
    })
    .expect("the playing process shows up in the app list");
    println!("listed: {app:?}");
    assert!(!app.name.is_empty());
    assert!(
        app.exe_path.as_ref().is_some_and(|p| p.ends_with("powershell.exe")),
        "{app:?}"
    );

    // Capturing the app gets the tone.
    let full = measure(&source, &app);
    println!("capture of the player: {full:?}");
    assert_tone(full, 0.5);

    // Capturing our own process includes the player, because it is our child.
    let parent = AudioApp {
        pid: std::process::id(),
        name: "test".into(),
        exe_path: None,
    };
    let tree = measure(&source, &parent);
    println!("capture of the parent: {tree:?}");
    assert_tone(tree, 0.5);

    // Ducking lowers the session volume and restores it exactly.
    let before = session_volumes(player_pid);
    assert!(!before.is_empty(), "the player has an audio session");
    source.set_app_volume(&app, 0.25).unwrap();
    let ducked_volumes = session_volumes(player_pid);
    let ducked = measure(&source, &app);
    // Ducking again must not compound.
    source.set_app_volume(&app, 0.25).unwrap();
    assert_eq!(session_volumes(player_pid), ducked_volumes);
    source.set_app_volume(&app, 1.0).unwrap();
    let after = session_volumes(player_pid);
    let restored = measure(&source, &app);

    for (b, d) in before.iter().zip(&ducked_volumes) {
        assert!((d - b * 0.25).abs() < 1e-3, "ducked {d}, before {b}");
    }
    assert_eq!(after, before);
    // The capture is compensated, so ducking doesn't change its level.
    println!("ducked capture: {ducked:?}, restored: {restored:?}");
    assert!(
        (ducked.rms / full.rms - 1.0).abs() < 0.05,
        "ducked capture at {:.2}×",
        ducked.rms / full.rms
    );
    assert_tone(ducked, 0.9);
    assert_tone(restored, 0.9);

    // Around each volume change, compensation and the actual volume can be 10 ms or more out of step. Ducking fades in
    // half-dB steps, so that leaves the capture a step or two off, rarely three.
    let profile = duck_transitions(&source, &app, full.rms);
    let mut off = Vec::new();
    for (ms, &ratio) in profile.iter().enumerate() {
        if (ratio - 1.0).abs() > 0.04 {
            match off.last_mut() {
                Some((start, len, lo, hi)) if *start + *len == ms => {
                    *len += 1;
                    *lo = ratio.min(*lo);
                    *hi = ratio.max(*hi);
                }
                _ => off.push((ms, 1, ratio, ratio)),
            }
        }
    }
    println!("1 ms windows off by more than 4% (start ms, length ms, min×, max×): {off:.2?}");
    let lo = profile.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = profile.iter().copied().fold(0.0, f64::max);
    let db = |ratio: f64| 20.0 * ratio.log10();
    println!(
        "worst 1 ms windows across {} ms of ducking and restoring: {lo:.3}× ({:+.2} dB), {hi:.3}× ({:+.2} dB)",
        profile.len(),
        db(lo),
        db(hi)
    );
    assert!(
        db(lo) >= -MAX_OFF_DB && db(hi) <= MAX_OFF_DB,
        "a 1 ms window is off by more than {MAX_OFF_DB} dB: {off:.2?}"
    );
}

#[test]
#[ignore = "plays audio; needs an audio output device and the probe example"]
fn the_duck_guard_restores_an_app_ducked_by_a_killed_process() {
    let _audio = AUDIO.lock().unwrap_or_else(|e| e.into_inner());
    // The probe ducks with a duck journal and guard, like the app.
    let probe = env::current_exe()
        .ok()
        .and_then(|test| Some(test.parent()?.parent()?.join("examples")))
        .unwrap_or_default()
        .join(format!("probe{}", env::consts::EXE_SUFFIX));
    assert!(
        probe.exists(),
        "{} is missing; build it with `cargo build -p rt-capture --example probe`",
        probe.display()
    );
    let journal = env::temp_dir().join("repeat-that-probe-duck.journal");
    let _ = std::fs::remove_file(&journal);

    let player = start_player();
    let player_pid = player.0.id();
    let before = wait_for(Duration::from_secs(30), || {
        Some(session_volumes(player_pid)).filter(|v| !v.is_empty())
    })
    .expect("the player has an audio session");

    // The probe holds the duck for a minute, far longer than the test needs.
    let prober = Killed(
        Command::new(&probe)
            .args(["duck", &player_pid.to_string(), "0.25", "60"])
            .spawn()
            .expect("start the probe"),
    );
    wait_for(Duration::from_secs(30), || {
        let now = session_volumes(player_pid);
        let ducked = now.len() == before.len() && now.iter().zip(&before).all(|(n, b)| (n - b * 0.25).abs() < 1e-3);
        ducked.then_some(())
    })
    .expect("the probe ducks the player to 25%");
    assert!(journal.exists(), "the probe keeps a duck journal while ducked");

    // Killed without a chance to restore anything itself, like a crash.
    let killed = Command::new("taskkill")
        .args(["/F", "/PID", &prober.0.id().to_string()])
        .status()
        .expect("run taskkill");
    assert!(killed.success(), "taskkill: {killed}");
    let start = Instant::now();
    let mut restored = None;
    while start.elapsed() < Duration::from_secs(10) {
        if session_volumes(player_pid) == before {
            restored = Some(start.elapsed());
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    println!(
        "duck guard: volume {before:?} restored {restored:?} after taskkill /F, journal left: {}",
        journal.exists()
    );
    let restored = restored.expect("the duck guard restores the player's volume");
    assert!(restored <= Duration::from_secs(2), "restored only after {restored:?}");
    wait_for(Duration::from_secs(2), || (!journal.exists()).then_some(()))
        .expect("the duck guard removes the journal once everything is restored");
}

#[test]
#[ignore = "needs the Windows shell and the system's powershell.exe"]
fn extracts_the_icon_of_an_executable() {
    let system = env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let powershell = system.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    assert!(powershell.exists(), "{} is missing", powershell.display());

    for size in [32, 64] {
        let icon = app_icon(&powershell, size).unwrap_or_else(|| panic!("no {size} px icon for powershell.exe"));
        assert_eq!((icon.width, icon.height), (size, size));
        assert_eq!(icon.rgba.len(), (size * size * 4) as usize);

        let pixels = icon.rgba.as_chunks::<4>().0;
        let visible = pixels.iter().filter(|p| p[3] > 0).count();
        let opaque = pixels.iter().filter(|p| p[3] == 255).count();
        let colored = pixels.iter().filter(|p| p[3] == 255 && p[..3] != [0; 3]).count();
        let percent = |n: usize| 100.0 * n as f64 / pixels.len() as f64;
        println!(
            "powershell.exe at {size} px: {:.0}% visible, {:.0}% opaque, {:.0}% opaque and not black",
            percent(visible),
            percent(opaque),
            percent(colored),
        );
        assert!(opaque > 0, "no opaque pixels at {size} px");
        // A lost color bitmap would leave the icon black.
        assert!(colored > 0, "all opaque pixels are black at {size} px");
    }

    // A file that isn't there has no icon.
    assert!(app_icon(&system.join("this-does-not-exist.exe"), 32).is_none());
}

/// Captures continuously while ducking to 25% and back [`FADES`] times, and returns the RMS of each 1 ms window (one
/// period of the tone) relative to `full_rms`, the level of an undisturbed capture.
fn duck_transitions(source: &WasapiSource, app: &AudioApp, full_rms: f64) -> Vec<f64> {
    let (tx, rx) = mpsc::channel::<(Vec<f32>, CaptureFormat)>();
    let handle = source
        .start(
            app,
            Box::new(move |frames, format| {
                let _ = tx.send((frames.to_vec(), format));
            }),
        )
        .expect("capture starts");
    thread::sleep(Duration::from_millis(500));
    for _ in 0..FADES {
        source.set_app_volume(app, 0.25).unwrap();
        thread::sleep(Duration::from_millis(300));
        source.set_app_volume(app, 1.0).unwrap();
        thread::sleep(Duration::from_millis(300));
    }
    handle.stop();

    let mut mono = Vec::new();
    let mut rate = 48_000;
    for (frames, format) in rx.try_iter() {
        rate = format.sample_rate as usize;
        mono.extend(
            frames
                .iter()
                .step_by(usize::from(format.channels))
                .map(|&s| f64::from(s)),
        );
    }
    // Skip the first 300 ms, where the stream may still be filling.
    let mono = &mono[(rate * 3 / 10).min(mono.len())..];
    mono.chunks_exact(rate / 1000)
        .map(|w| (w.iter().map(|s| s * s).sum::<f64>() / w.len() as f64).sqrt() / full_rms)
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct Measurement {
    rms: f64,
    hz: f64,
    seconds: f64,
}

/// Captures `app` for a second (after a short settle time) and measures level and frequency of the first channel.
fn measure(source: &WasapiSource, app: &AudioApp) -> Measurement {
    let (tx, rx) = mpsc::channel::<(Vec<f32>, CaptureFormat)>();
    let handle = source
        .start(
            app,
            Box::new(move |frames, format| {
                let _ = tx.send((frames.to_vec(), format));
            }),
        )
        .expect("capture starts");
    thread::sleep(Duration::from_millis(1300));
    assert!(handle.is_running());
    handle.stop();

    let mut mono = Vec::new();
    let mut rate = 0;
    for (frames, format) in rx.try_iter() {
        rate = format.sample_rate;
        mono.extend(
            frames
                .iter()
                .step_by(usize::from(format.channels))
                .map(|&s| f64::from(s)),
        );
    }
    assert!(rate > 0, "no audio captured");
    // Skip the start, where the stream may still be filling.
    let settle = (rate as usize * 3 / 10).min(mono.len());
    let mono = &mono[settle..];
    let seconds = mono.len() as f64 / f64::from(rate);
    let rms = (mono.iter().map(|s| s * s).sum::<f64>() / mono.len().max(1) as f64).sqrt();
    let rising = mono.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    Measurement {
        rms,
        hz: rising as f64 / seconds.max(1e-9),
        seconds,
    }
}

/// At least `min_level` of the tone's full level, at the tone's frequency.
fn assert_tone(m: Measurement, min_level: f64) {
    assert!(m.seconds > 0.5, "only {:.2} s captured", m.seconds);
    assert!(m.rms > TONE_RMS * min_level, "level {:.3}", m.rms);
    assert!((m.hz - TONE_HZ).abs() < TONE_HZ * 0.05, "frequency {:.0} Hz", m.hz);
}

fn wait_for<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(value) = f() {
            return Some(value);
        }
        thread::sleep(Duration::from_millis(250));
    }
    None
}

/// The volume of each audio session of `pid`, read independently of the backend.
fn session_volumes(pid: u32) -> Vec<f32> {
    use windows::Win32::Media::Audio::{
        DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
        MMDeviceEnumerator, eRender,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::core::Interface;

    let mut volumes = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).unwrap();
        for d in 0..devices.GetCount().unwrap() {
            let manager: IAudioSessionManager2 = devices.Item(d).unwrap().Activate(CLSCTX_ALL, None).unwrap();
            let sessions = manager.GetSessionEnumerator().unwrap();
            for s in 0..sessions.GetCount().unwrap() {
                let session: IAudioSessionControl2 = sessions.GetSession(s).unwrap().cast().unwrap();
                if session.GetProcessId().ok() == Some(pid) {
                    volumes.push(session.cast::<ISimpleAudioVolume>().unwrap().GetMasterVolume().unwrap());
                }
            }
        }
    }
    volumes
}
