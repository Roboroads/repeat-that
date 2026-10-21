//! Development tool: tries the capture backend of this OS on a real app. No audio is written to disk; the duck journal
//! (app volumes only) lives in the temp directory.
//!
//! ```sh
//! cargo run -p rt-capture --example probe -- list
//! cargo run -p rt-capture --example probe -- capture <pid or name> [seconds] [--duck]
//! cargo run -p rt-capture --example probe -- duck <pid or name> [level] [seconds]
//! cargo run -p rt-capture --example probe -- icon <pid, name or exe path> [size in pixels]
//! ```
//!
//! `capture` prints the level once a second and every utterance the VAD finds. With `--duck` it also ducks the app to
//! 20% from second 5 to 10, which shows whether ducking changes what is captured. `icon` prints the size of an app's
//! icon and how much of it is not transparent; it writes no file.

use std::env;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rt_capture::{AppAudioSource, AudioApp, CaptureFormat, PlatformOptions};
use rt_core::{AudioFormat, Recorder, SegmenterConfig, SileroVad};

type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    if rt_capture::duck_guard::run_if_requested() {
        return Ok(());
    }
    let args: Vec<String> = env::args().skip(1).collect();
    let source = source()?;
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["list"] => list(source.as_ref()),
        ["capture", app, rest @ ..] => {
            let duck = rest.contains(&"--duck");
            let secs = rest.iter().find_map(|s| s.parse().ok()).unwrap_or(20);
            capture(source.as_ref(), &find(source.as_ref(), app)?, secs, duck)
        }
        ["duck", app, rest @ ..] => {
            let level = rest.first().map(|s| s.parse()).transpose()?.unwrap_or(0.2);
            let secs = rest.get(1).map(|s| s.parse()).transpose()?.unwrap_or(3);
            duck(source.as_ref(), &find(source.as_ref(), app)?, level, secs)
        }
        ["icon", app, rest @ ..] => {
            let size = rest.first().map(|s| s.parse()).transpose()?.unwrap_or(32);
            icon(source.as_ref(), app, size)
        }
        _ => {
            eprintln!(
                "usage: probe list | capture <pid|name> [seconds] [--duck] | duck <pid|name> [level] [seconds] \
                 | icon <pid|name|exe path> [size]"
            );
            std::process::exit(2);
        }
    }
}

/// The capture backend, with a duck journal (and a guard, except in a Flatpak) so a probe killed mid-duck
/// (`taskkill /F`, `kill -9`) gives the app its volume back.
fn source() -> Result<Box<dyn AppAudioSource>, Error> {
    let journal = env::temp_dir().join("repeat-that-probe-duck.journal");
    Ok(rt_capture::platform_source_with(PlatformOptions {
        duck_journal: Some(journal),
    })?)
}

fn list(source: &dyn AppAudioSource) -> Result<(), Error> {
    for app in source.list_apps()? {
        let exe = app
            .exe_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        println!("{:>7}  {:<30} {exe}", app.pid, app.name);
    }
    Ok(())
}

/// Finds an app by PID or by a case-insensitive part of its name.
fn find(source: &dyn AppAudioSource, query: &str) -> Result<AudioApp, Error> {
    let apps = source.list_apps()?;
    let pid: Option<u32> = query.parse().ok();
    let query = query.to_lowercase();
    apps.into_iter()
        .find(|app| Some(app.pid) == pid || app.name.to_lowercase().contains(&query))
        .ok_or_else(|| format!("no app matches {query:?}; see `probe list`").into())
}

fn capture(source: &dyn AppAudioSource, app: &AudioApp, secs: u64, duck: bool) -> Result<(), Error> {
    println!("capturing {} (PID {}) for {secs} s", app.name, app.pid);
    let (tx, rx) = mpsc::sync_channel::<(Vec<f32>, CaptureFormat)>(256);
    let handle = source.start(
        app,
        Box::new(move |frames, format| {
            // Dropping frames is fine for a probe; the sink must never block.
            let _ = tx.try_send((frames.to_vec(), format));
        }),
    )?;

    let start = Instant::now();
    let mut recorder: Option<Recorder> = None;
    let mut rate = 0;
    let (mut sum_sq, mut count, mut frames_total) = (0.0f64, 0usize, 0usize);
    let mut next_report = Duration::from_secs(1);
    let mut ducked = false;
    while start.elapsed() < Duration::from_secs(secs) {
        let duck_time = (Duration::from_secs(5)..Duration::from_secs(10)).contains(&start.elapsed());
        if duck && !ducked && duck_time {
            source.set_app_volume(app, 0.2)?;
            ducked = true;
            println!("-- ducked to 20%");
        }
        if duck && ducked && !duck_time {
            source.set_app_volume(app, 1.0)?;
            ducked = false;
            println!("-- volume restored");
        }
        if let Ok((frames, format)) = rx.recv_timeout(Duration::from_millis(50)) {
            let recorder = recorder.get_or_insert_with(|| {
                println!("format: {} Hz, {} channels", format.sample_rate, format.channels);
                rate = format.sample_rate;
                let vad = Box::new(SileroVad::new().expect("Silero VAD loads"));
                let format = AudioFormat {
                    sample_rate: format.sample_rate,
                    channels: format.channels,
                };
                Recorder::new(format, vad, SegmenterConfig::default(), Duration::from_secs(60))
            });
            sum_sq += frames.iter().map(|&s| f64::from(s * s)).sum::<f64>();
            count += frames.len();
            frames_total += frames.len() / usize::from(format.channels);
            if recorder.push(&frames) > 0 {
                let buffer = recorder.buffer();
                // Not the latest utterance: after a split that's the part that just opened.
                let latest = buffer
                    .latest_finished_id()
                    .and_then(|id| buffer.get(id))
                    .expect("an utterance just finished");
                println!(
                    "   utterance #{} {:.2} s ({} buffered, {:.1} s of speech)",
                    latest.id,
                    latest.samples.len() as f32 / format.sample_rate as f32,
                    buffer.len(),
                    buffer.speech_duration().as_secs_f32()
                );
            }
        }
        if start.elapsed() >= next_report {
            let rms = if count > 0 { (sum_sq / count as f64).sqrt() } else { 0.0 };
            let dbfs = 20.0 * rms.max(1e-9).log10();
            let bar = "#".repeat(((dbfs + 60.0).max(0.0) / 2.0) as usize);
            println!("{:>3} s {dbfs:>6.1} dBFS {bar}", next_report.as_secs());
            (sum_sq, count) = (0.0, 0);
            next_report += Duration::from_secs(1);
        }
        if !handle.is_running() {
            println!("capture stopped by itself");
            break;
        }
    }
    if ducked {
        source.set_app_volume(app, 1.0)?;
    }
    handle.stop();
    if rate > 0 {
        println!("received {:.1} s of audio", frames_total as f64 / f64::from(rate));
    } else {
        println!("received no audio");
    }
    Ok(())
}

fn duck(source: &dyn AppAudioSource, app: &AudioApp, level: f32, secs: u64) -> Result<(), Error> {
    println!(
        "ducking {} (PID {}) to {:.0}% for {secs} s",
        app.name,
        app.pid,
        level * 100.0
    );
    source.set_app_volume(app, level)?;
    thread::sleep(Duration::from_secs(secs));
    source.set_app_volume(app, 1.0)?;
    println!("volume restored");
    Ok(())
}

/// Prints the size of the icon of an app, or of the executable at `query`, and the share of its pixels that are not
/// transparent. Writes nothing.
fn icon(source: &dyn AppAudioSource, query: &str, size: u32) -> Result<(), Error> {
    let exe = if Path::new(query).is_file() {
        query.into()
    } else {
        let app = find(source, query)?;
        app.exe_path
            .ok_or_else(|| format!("{} (PID {}) has no known executable", app.name, app.pid))?
    };
    let icon = rt_capture::app_icon(&exe, size).ok_or_else(|| format!("no {size} px icon for {}", exe.display()))?;
    let pixels = icon.rgba.as_chunks::<4>().0;
    let percent =
        |keep: fn(&[u8; 4]) -> bool| 100.0 * pixels.iter().filter(|p| keep(p)).count() as f64 / pixels.len() as f64;
    println!(
        "{}: {}x{} icon, {:.0}% of the pixels are not transparent, {:.0}% fully opaque",
        exe.display(),
        icon.width,
        icon.height,
        percent(|p| p[3] > 0),
        percent(|p| p[3] == 255),
    );
    Ok(())
}
