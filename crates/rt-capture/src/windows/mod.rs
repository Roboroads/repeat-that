//! Windows backend: audio sessions for the app list and ducking, WASAPI process loopback for capture.

mod loopback;
mod sessions;

use std::collections::HashMap;
use std::sync::Mutex;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::{AppAudioSource, AudioApp, CaptureError, CaptureHandle, DuckLevel, FrameSink, Result, clamp_duck_level};

/// Captures one app through WASAPI process loopback (Windows 10 2004+), including its child processes.
///
/// Process loopback captures the app after its own volume is applied, so ducking would make the capture quieter too.
/// Captures of a ducked app are amplified by the inverse of the duck level to keep their level.
#[derive(Default)]
pub struct WasapiSource {
    /// Volume of each ducked audio session before ducking, by session instance id.
    before_duck: Mutex<HashMap<String, f32>>,
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
        loopback::start(app.pid, self.level(app.pid), sink)
    }

    fn set_app_volume(&self, app: &AudioApp, level: f32) -> Result<()> {
        let _com = Com::init()?;
        let mut before_duck = self.before_duck.lock().unwrap_or_else(|e| e.into_inner());
        let level = clamp_duck_level(level);
        sessions::duck(app.pid, level, &mut before_duck)?;
        // Only compensate once the volume has actually changed.
        self.level(app.pid).set(level);
        Ok(())
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
