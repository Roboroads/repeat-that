//! Wire types of the contract with the settings window (`app/src/lib/api.ts` has the TypeScript side): settings,
//! status, apps and output devices. Everything is serde `camelCase`.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

/// `Settings::version` today. Stored settings of another version are read field by field like any other.
pub const SETTINGS_VERSION: u32 = 1;

/// How readily the voice detector calls something speech.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    Low,
    #[default]
    Normal,
    High,
}

/// The app whose audio is captured. `exe_path` is what identifies it across restarts, `name` is for display and for
/// matching when there is no path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    #[serde(default)]
    pub exe_path: Option<String>,
    pub name: String,
}

/// Lowering the target app's volume while a replay plays.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Duck {
    pub enabled: bool,
    pub level: f64,
}

/// How many actions have a hotkey. The table of them is `HOTKEYS` in `settings.rs`; the `slots` methods of the
/// types below list their fields in the same order, and a test keeps the two in step.
pub const HOTKEY_COUNT: usize = 3;

/// One accelerator per action, like `"Ctrl+Alt+Space"`; `None` = unbound.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyBindings {
    pub replay: Option<String>,
    pub stop: Option<String>,
    pub catch_up: Option<String>,
}

impl HotkeyBindings {
    pub fn slots(&self) -> [&Option<String>; HOTKEY_COUNT] {
        [&self.replay, &self.stop, &self.catch_up]
    }

    pub fn slots_mut(&mut self) -> [&mut Option<String>; HOTKEY_COUNT] {
        [&mut self.replay, &mut self.stop, &mut self.catch_up]
    }
}

/// Everything the user can set. Owned by the Rust side; the window edits it through `update_settings`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub version: u32,
    pub target: Option<Target>,
    pub buffer_seconds: u32,
    pub sensitivity: Sensitivity,
    /// `None` = the system default output.
    pub output_device: Option<String>,
    pub replay_speed: f64,
    pub catch_up_speed: f64,
    pub duck: Duck,
    pub hotkeys: HotkeyBindings,
    pub autostart: bool,
    pub first_run_done: bool,
}

/// Whether an action's hotkey is live.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HotkeyState {
    /// Registered with the OS.
    Ok,
    /// The OS refused it, most likely because another app holds the combination.
    InUse,
    /// The accelerator can't be understood.
    Invalid,
    /// No combination chosen.
    Unbound,
}

/// The state of each hotkey, as `Status.hotkeys` carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyStates {
    pub replay: HotkeyState,
    pub stop: HotkeyState,
    pub catch_up: HotkeyState,
}

impl HotkeyStates {
    pub fn slots(&self) -> [HotkeyState; HOTKEY_COUNT] {
        [self.replay, self.stop, self.catch_up]
    }

    pub fn slots_mut(&mut self) -> [&mut HotkeyState; HOTKEY_COUNT] {
        [&mut self.replay, &mut self.stop, &mut self.catch_up]
    }
}

/// What capture is doing, `Status.capture.state`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureStateName {
    #[default]
    NoTarget,
    /// The target isn't running; looked for every 2 s.
    Waiting,
    Capturing,
    Paused,
    /// Capture failed to start; tried again every 2 s.
    Error,
    /// Capturing isn't possible here.
    Unsupported,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CaptureStatus {
    pub state: CaptureStateName,
    /// Why, for `error` and `unsupported`.
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Playback {
    #[default]
    Idle,
    Replaying,
    CatchingUp,
}

/// The output of the latest replay.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputStatus {
    pub device_name: Option<String>,
    /// The chosen device was missing or failed, so the system default played.
    pub fell_back: bool,
    /// Nothing could play.
    pub error: Option<String>,
}

/// Everything the settings window and the tray show about what the app is doing. Sent with the `status` event on every
/// change, and every 500 ms during a replay.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub capture: CaptureStatus,
    /// The target's name, also while it isn't running.
    pub target_name: Option<String>,
    pub playback: Playback,
    /// How far the replay is from live; `None` while idle.
    pub behind_ms: Option<u64>,
    /// Speech in the buffer.
    pub buffered_ms: u64,
    pub output: OutputStatus,
    pub hotkeys: HotkeyStates,
    /// The speech detection model didn't load, so a simpler detector is used.
    pub vad_fallback: bool,
    /// Why the settings can't be saved, in words for the user; `None` when they can.
    pub settings_problem: Option<String>,
}

/// An app that plays audio, for the app list. `select_app` gets one back, usually without its icon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEntry {
    pub pid: u32,
    pub name: String,
    #[serde(default)]
    pub exe_path: Option<String>,
    /// A `data:` URL of a PNG; `None` for the generic icon.
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDevice {
    /// What `Settings.outputDevice` stores.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Why a command failed. Serialized as `{ kind, message }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandErrorKind {
    /// The input is malformed or unusable (an unparseable hotkey, a wrong type).
    Invalid,
    /// The input clashes with the current state (two actions on one hotkey).
    Conflict,
    /// The input was fine but the system couldn't do it (saving the file, the autostart entry).
    Backend,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommandError {
    pub kind: CommandErrorKind,
    pub message: String,
}

impl CommandError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(CommandErrorKind::Invalid, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(CommandErrorKind::Conflict, message)
    }

    pub fn backend(message: impl Into<String>) -> Self {
        Self::new(CommandErrorKind::Backend, message)
    }

    fn new(kind: CommandErrorKind, message: impl Into<String>) -> Self {
        CommandError {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

/// For a field where `null` means something (no target, system default output, unbound): absent = `None` (leave it),
/// `null` = `Some(None)` (clear it), a value = `Some(Some(value))`.
fn clearable<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// The argument of `update_settings`: only the fields to change. Unknown fields (and `version`) are ignored.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    #[serde(default, deserialize_with = "clearable")]
    pub target: Option<Option<Target>>,
    pub buffer_seconds: Option<f64>,
    pub sensitivity: Option<Sensitivity>,
    #[serde(default, deserialize_with = "clearable")]
    pub output_device: Option<Option<String>>,
    pub replay_speed: Option<f64>,
    pub catch_up_speed: Option<f64>,
    pub duck: Option<DuckPatch>,
    pub hotkeys: Option<HotkeysPatch>,
    pub autostart: Option<bool>,
    pub first_run_done: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuckPatch {
    pub enabled: Option<bool>,
    pub level: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeysPatch {
    #[serde(default, deserialize_with = "clearable")]
    pub replay: Option<Option<String>>,
    #[serde(default, deserialize_with = "clearable")]
    pub stop: Option<Option<String>>,
    #[serde(default, deserialize_with = "clearable")]
    pub catch_up: Option<Option<String>>,
}

impl HotkeysPatch {
    /// The change asked for each hotkey, in the order of the table in `settings.rs`.
    pub fn into_changes(self) -> [Option<Option<String>>; HOTKEY_COUNT] {
        [self.replay, self.stop, self.catch_up]
    }
}
