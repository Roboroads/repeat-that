//! End-to-end test of the Windows backend against a real audio device: a child process plays a tone, and we list,
//! capture and duck it. CI installs a virtual audio device for this; run it locally with
//! `cargo test -p rt-capture --test windows_audio -- --ignored --nocapture`.
//!
//! The tone is synthetic and everything stays in memory.

#![cfg(windows)]

use std::process::{Child, Command};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rt_capture::{AppAudioSource, AudioApp, CaptureFormat, WasapiSource};

const TONE_HZ: f64 = 1000.0;
/// The player's sine has an amplitude of 0.5, so an RMS of about 0.35 at full volume.
const TONE_RMS: f64 = 0.354;

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

/// Kills the player when the test ends, also on a failed assertion.
struct Player(Child);

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "plays audio; needs an audio output device"]
fn lists_captures_and_ducks_a_playing_process() {
    let source = WasapiSource::new();
    let player = Player(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", PLAYER])
            .spawn()
            .expect("start PowerShell"),
    );
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
    println!(
        "captured level while ducked: {:.2}× of normal ({}), after restore {:.2}×",
        ducked.rms / full.rms,
        if ducked.rms / full.rms < 0.5 {
            "ducking DOES lower what we capture"
        } else {
            "ducking does NOT change what we capture"
        },
        restored.rms / full.rms,
    );
    assert_tone(restored, 0.5);
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
