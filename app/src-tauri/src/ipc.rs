//! Wire types of the settings contract (`app/src/lib/api.ts` has the TypeScript side). Everything is serde `camelCase`.
//! The status types join these with the audio engine.

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
    pub fn slots_mut(&mut self) -> [&mut HotkeyState; HOTKEY_COUNT] {
        [&mut self.replay, &mut self.stop, &mut self.catch_up]
    }
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
