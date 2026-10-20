//! Windows backend: audio sessions for the app list and ducking, WASAPI process loopback for capture.

mod loopback;
mod sessions;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::before_duck::BeforeDuck;
use crate::{AppAudioSource, AudioApp, CaptureError, CaptureHandle, DuckLevel, FrameSink, Result, clamp_duck_level};

/// Captures one app through WASAPI process loopback (Windows 10 2004+), including its child processes.
///
/// Process loopback captures the app after its own volume is applied, so ducking would make the capture quieter too.
/// Captures of a ducked app are amplified by the inverse of the duck level to keep their level.
///
/// Apps start new audio sessions while ducked (a new voice stream, a new tab, a switch to another device). Those would
/// play at full volume and then be boosted too, so a running capture ducks its app's new sessions every 500 ms on its
/// own thread, which already has COM. Without a capture there is no boost, so nothing else needs to watch for new
/// sessions.
#[derive(Default)]
pub struct WasapiSource {
    /// Volume of each ducked audio session before ducking, shared with captures that duck new sessions.
    before_duck: Arc<Mutex<BeforeDuck>>,
    /// Current duck level of each app, by PID, shared with its captures.
    levels: Mutex<HashMap<u32, DuckLevel>>,
}

impl WasapiSource {
    pub fn new() -> Self {
        Self::default()
    }

    fn level(&self, pid: u32) -> DuckLevel {
        let mut levels = self.levels.lock().unwrap_or_else(|e| e.into_inner());
        levels.entry(pid).or_insert_with(DuckLevel::new).clone()
    }
}

impl AppAudioSource for WasapiSource {
    fn list_apps(&self) -> Result<Vec<AudioApp>> {
        let _com = Com::init()?;
        sessions::list_apps()
    }

    fn start(&self, app: &AudioApp, sink: FrameSink) -> Result<Box<dyn CaptureHandle>> {
        let (pid, level, before_duck) = (app.pid, self.level(app.pid), Arc::clone(&self.before_duck));
        let duck_level = level.clone();
        let reduck = move || {
            // Locked before reading the level, like `set_app_volume` holds it while changing the level, so this can't
            // duck sessions an unduck just restored.
            let mut before_duck = before_duck.lock().unwrap_or_else(|e| e.into_inner());
            let level = duck_level.get().level;
            if level < 1.0 {
                let _ = sessions::duck_new_sessions(pid, level, &mut before_duck);
            }
        };
        loopback::start(pid, level, sink, Box::new(reduck))
    }

    fn set_app_volume(&self, app: &AudioApp, level: f32) -> Result<()> {
        let _com = Com::init()?;
        let mut before_duck = self.before_duck.lock().unwrap_or_else(|e| e.into_inner());
        let level = clamp_duck_level(level);
        sessions::duck(app.pid, level, &mut before_duck)?;
        // Captures switch their compensation at this moment, for audio the engine processed from then on.
        self.level(app.pid).set(level, loopback::volume_takes_effect());
        Ok(())
    }
}

impl Drop for WasapiSource {
    /// Restores every app that is still ducked. Windows remembers per-app volumes, so it would stay quiet otherwise.
    fn drop(&mut self) {
        let mut before_duck = self.before_duck.lock().unwrap_or_else(|e| e.into_inner());
        if before_duck.is_empty() {
            return;
        }
        // Captures that outlive the source stop ducking new sessions and boosting.
        let levels = self.levels.lock().unwrap_or_else(|e| e.into_inner());
        for level in levels.values() {
            level.set(1.0, loopback::now());
        }
        if let Ok(_com) = Com::init() {
            let _ = sessions::restore_all(&mut before_duck);
        }
    }
}

/// COM for the current thread, for as long as this lives. Threads that already use COM, like a UI thread in a
/// single-threaded apartment, are left as they are.
pub(crate) struct Com {
    uninit: bool,
}

impl Com {
    pub(crate) fn init() -> Result<Self> {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            return Ok(Self { uninit: false });
        }
        hr.ok().map_err(backend("CoInitializeEx"))?;
        Ok(Self { uninit: true })
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.uninit {
            unsafe { CoUninitialize() };
        }
    }
}

/// Maps a Windows error to a [`CaptureError`] that says which call failed.
pub(crate) fn backend(call: &'static str) -> impl Fn(windows::core::Error) -> CaptureError {
    move |e| CaptureError::Backend(format!("{call}: {e}"))
}
