//! Development tool: runs the audio engine on a real app, to try replays, ducking and catch-up by ear. No audio is
//! written to disk; the duck journal (app volumes only) lives in the temp directory. On an OS without a capture
//! backend, the rt-core speech fixture plays as a mock app.
//!
//! ```sh
//! cargo run -p rt-engine --example engine_probe -- list
//! cargo run -p rt-engine --example engine_probe -- <pid or name> [--null]
//! ```
//!
//! Then type a command and press Enter: nothing replays (again: one sentence further back), `c` toggles catch-up, `s`
//! stops, `p` pauses or resumes capture, `q` quits. Every status change is printed. `--null` plays into nothing, for
//! machines without an audio device.

use std::env;
use std::io::{self, BufRead};
use std::sync::Arc;
use std::thread;

use rt_capture::{AppAudioSource, AudioApp, PlatformOptions};
use rt_engine::{CpalOutput, Engine, EngineConfig, NullOutput, OutputBackend, Status, TargetApp};

type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    if rt_capture::duck_guard::run_if_requested() {
        return Ok(());
    }
    let args: Vec<String> = env::args().skip(1).collect();
    let null = args.iter().any(|a| a == "--null");
    let source = source()?;
    match args.iter().find(|a| !a.starts_with("--")).map(String::as_str) {
        Some("list") => list(source.as_ref()),
        Some(query) => run(source, query, null),
        None => {
            eprintln!("usage: engine_probe list | <pid|name> [--null]");
            std::process::exit(2);
        }
    }
}

/// The capture backend, with a duck journal like the app (and on Windows a guard), so a probe killed mid-replay gives
/// the app its volume back. Where there is no backend, the speech fixture plays as a fake app.
fn source() -> Result<Arc<dyn AppAudioSource>, Error> {
    if !rt_capture::HAS_PLATFORM_BACKEND {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../rt-core/tests/fixtures/speech_8s.wav");
        return Ok(Arc::new(rt_capture::MockSource::from_wav(fixture)?));
    }
    let journal = env::temp_dir().join("repeat-that-engine-probe-duck.journal");
    Ok(Arc::from(rt_capture::platform_source_with(PlatformOptions {
        duck_journal: Some(journal),
    })?))
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
        .ok_or_else(|| format!("no app matches {query:?}; see `engine_probe list`").into())
}

fn run(source: Arc<dyn AppAudioSource>, query: &str, null: bool) -> Result<(), Error> {
    let app = find(source.as_ref(), query)?;
    let output: Box<dyn OutputBackend> = if null {
        Box::new(NullOutput::default())
    } else {
        Box::new(CpalOutput::new())
    };
    let engine = Engine::new(source, output, EngineConfig::default());
    let handle = engine.handle();
    for device in handle.list_output_devices()? {
        let default = if device.is_default { " (default)" } else { "" };
        println!("output: {}{default}  [{}]", device.name, device.id);
    }
    println!(
        "capturing {} (PID {}); Enter replays, c catch-up, s stop, p pause, q quit",
        app.name, app.pid
    );
    handle.set_target(Some(TargetApp::from(&app)));

    let updates = handle.subscribe();
    thread::spawn(move || {
        for status in updates {
            print_status(&status);
        }
    });

    let mut paused = false;
    for line in io::stdin().lock().lines() {
        match line?.trim() {
            "" => handle.replay(),
            "c" => handle.toggle_catch_up(),
            "s" => handle.stop(),
            "p" => {
                paused = !paused;
                handle.set_paused(paused);
            }
            "q" => break,
            other => println!("unknown command {other:?}"),
        }
    }
    drop(engine);
    println!("stopped; the buffer is wiped and the app's volume restored");
    Ok(())
}

fn print_status(status: &Status) {
    let behind = status
        .behind
        .map(|b| format!(", {:.1} s behind live", b.as_secs_f32()))
        .unwrap_or_default();
    let output = match (&status.output.error, &status.output.device_name) {
        (Some(error), _) => format!(", output error: {error}"),
        (None, Some(name)) if status.output.fell_back => format!(", on {name} (fallback)"),
        (None, Some(name)) => format!(", on {name}"),
        (None, None) => String::new(),
    };
    let vad = if status.vad_fallback { ", energy VAD" } else { "" };
    println!(
        "{:?} {} · {:.1} s of speech · {:?}{behind}{output}{vad}",
        status.capture,
        status.target_name.as_deref().unwrap_or("-"),
        status.buffered.as_secs_f32(),
        status.playback,
    );
}
