//! Settings: defaults, reading stored values field by field, applying a patch from the window.

use serde_json::{Map, Value};
use tauri_plugin_global_shortcut::Shortcut;

use crate::actions::AppAction;
use crate::ipc::{CommandError, Duck, HotkeyBindings, SETTINGS_VERSION, Sensitivity, Settings, SettingsPatch, Target};

pub const BUFFER_SECONDS_MIN: u32 = 10;
pub const BUFFER_SECONDS_MAX: u32 = 120;
const BUFFER_SECONDS_DEFAULT: u32 = 30;
const REPLAY_SPEED: (f64, f64, f64) = (0.75, 1.5, 1.0);
const CATCH_UP_SPEED: (f64, f64, f64) = (1.25, 2.0, 1.5);
const DUCK_LEVEL: (f64, f64, f64) = (0.05, 1.0, 0.2);

impl Default for HotkeyBindings {
    fn default() -> Self {
        HotkeyBindings {
            replay: Some("Ctrl+Alt+Space".into()),
            stop: Some("Ctrl+Alt+Backspace".into()),
            catch_up: Some("Ctrl+Alt+Enter".into()),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: SETTINGS_VERSION,
            target: None,
            buffer_seconds: BUFFER_SECONDS_DEFAULT,
            sensitivity: Sensitivity::Normal,
            output_device: None,
            replay_speed: REPLAY_SPEED.2,
            catch_up_speed: CATCH_UP_SPEED.2,
            duck: Duck {
                enabled: true,
                level: DUCK_LEVEL.2,
            },
            hotkeys: HotkeyBindings::default(),
            autostart: false,
            first_run_done: false,
        }
    }
}

/// The actions that have a hotkey, in the order they win a clash when stored settings contain a duplicate.
pub const HOTKEY_ACTIONS: [AppAction; 3] = [AppAction::Replay, AppAction::Stop, AppAction::CatchUp];

impl HotkeyBindings {
    /// The accelerator bound to `action`; `None` when unbound or when the action has no hotkey.
    pub fn accel(&self, action: AppAction) -> Option<&str> {
        match action {
            AppAction::Replay => self.replay.as_deref(),
            AppAction::Stop => self.stop.as_deref(),
            AppAction::CatchUp => self.catch_up.as_deref(),
            AppAction::TogglePause | AppAction::OpenSettings | AppAction::Quit => None,
        }
    }

    fn set(&mut self, action: AppAction, accel: Option<String>) {
        match action {
            AppAction::Replay => self.replay = accel,
            AppAction::Stop => self.stop = accel,
            AppAction::CatchUp => self.catch_up = accel,
            AppAction::TogglePause | AppAction::OpenSettings | AppAction::Quit => {}
        }
    }
}

/// What the user calls a hotkey action, for error messages.
fn action_label(action: AppAction) -> &'static str {
    match action {
        AppAction::Replay => "Replay",
        AppAction::Stop => "Stop",
        AppAction::CatchUp => "Catch-up",
        AppAction::TogglePause => "Pause",
        AppAction::OpenSettings => "Settings",
        AppAction::Quit => "Quit",
    }
}

/// The keys of the `hotkeys` object, in the order of [`HOTKEY_ACTIONS`].
const HOTKEY_FIELDS: [&str; HOTKEY_ACTIONS.len()] = ["replay", "stop", "catchUp"];

/// Parses an accelerator like `"Ctrl+Alt+Space"`. The error is a reason fit for the settings window.
pub fn parse_accel(accel: &str) -> Result<Shortcut, String> {
    let accel = accel.trim();
    if accel.is_empty() {
        return Err("it is empty".into());
    }
    accel
        .parse::<Shortcut>()
        .map_err(|_| "it isn't a key combination like Ctrl+Alt+Space".to_string())
}

fn clamp_range(value: f64, (min, max, _): (f64, f64, f64)) -> f64 {
    value.clamp(min, max)
}

fn clamp_buffer(seconds: f64) -> u32 {
    seconds
        .round()
        .clamp(BUFFER_SECONDS_MIN as f64, BUFFER_SECONDS_MAX as f64) as u32
}

fn read_number(value: Option<&Value>, range: (f64, f64, f64)) -> f64 {
    match value.and_then(Value::as_f64) {
        Some(number) if number.is_finite() => clamp_range(number, range),
        _ => range.2,
    }
}

fn read_bool(value: Option<&Value>, default: bool) -> bool {
    value.and_then(Value::as_bool).unwrap_or(default)
}

fn read_target(value: Option<&Value>) -> Option<Target> {
    let object = value?.as_object()?;
    let name = object.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let exe_path = object
        .get("exePath")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(String::from);
    Some(Target {
        exe_path,
        name: name.to_string(),
    })
}

fn read_hotkeys(value: Option<&Value>) -> HotkeyBindings {
    let defaults = HotkeyBindings::default();
    let stored = value.and_then(Value::as_object);
    let mut bindings = HotkeyBindings {
        replay: None,
        stop: None,
        catch_up: None,
    };
    let mut taken: Vec<Shortcut> = Vec::new();
    for (action, field) in HOTKEY_ACTIONS.into_iter().zip(HOTKEY_FIELDS) {
        let candidate = match stored.and_then(|object| object.get(field)) {
            Some(Value::Null) => None,
            Some(Value::String(accel)) if parse_accel(accel).is_ok() => Some(accel.trim().to_string()),
            _ => defaults.accel(action).map(String::from),
        };
        // Whatever clashes with an earlier action is left unbound rather than guessed at.
        let accepted = candidate.filter(|accel| match parse_accel(accel) {
            Ok(shortcut) if !taken.contains(&shortcut) => {
                taken.push(shortcut);
                true
            }
            _ => false,
        });
        bindings.set(action, accepted);
    }
    bindings
}

/// Settings from whatever the file held: every field that is missing or of the wrong type falls back to its default,
/// numbers are clamped into their range and a hotkey that can't be used is dropped. Always returns usable settings.
pub fn sanitize(stored: &Value) -> Settings {
    let defaults = Settings::default();
    let Some(object) = stored.as_object() else {
        return defaults;
    };
    let empty = Map::new();
    let duck = object.get("duck").and_then(Value::as_object).unwrap_or(&empty);
    Settings {
        version: SETTINGS_VERSION,
        target: read_target(object.get("target")),
        buffer_seconds: object
            .get("bufferSeconds")
            .and_then(Value::as_f64)
            .filter(|seconds| seconds.is_finite())
            .map_or(defaults.buffer_seconds, clamp_buffer),
        sensitivity: object
            .get("sensitivity")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or(defaults.sensitivity),
        output_device: object
            .get("outputDevice")
            .and_then(Value::as_str)
            .filter(|device| !device.is_empty())
            .map(String::from),
        replay_speed: read_number(object.get("replaySpeed"), REPLAY_SPEED),
        catch_up_speed: read_number(object.get("catchUpSpeed"), CATCH_UP_SPEED),
        duck: Duck {
            enabled: read_bool(duck.get("enabled"), defaults.duck.enabled),
            level: read_number(duck.get("level"), DUCK_LEVEL),
        },
        hotkeys: read_hotkeys(object.get("hotkeys")),
        autostart: read_bool(object.get("autostart"), defaults.autostart),
        first_run_done: read_bool(object.get("firstRunDone"), defaults.first_run_done),
    }
}

fn finite(value: f64, field: &str) -> Result<f64, CommandError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(CommandError::invalid(format!("{field} must be a number")))
    }
}

/// `current` with `patch` applied. Numbers are clamped; a malformed hotkey or two actions on one hotkey are errors and
/// nothing is applied.
pub fn apply_patch(current: &Settings, patch: SettingsPatch) -> Result<Settings, CommandError> {
    let mut next = current.clone();

    if let Some(target) = patch.target {
        next.target = match target {
            None => None,
            Some(target) => {
                let name = target.name.trim();
                if name.is_empty() {
                    return Err(CommandError::invalid("The app needs a name"));
                }
                Some(Target {
                    exe_path: target.exe_path.filter(|path| !path.is_empty()),
                    name: name.to_string(),
                })
            }
        };
    }
    if let Some(seconds) = patch.buffer_seconds {
        next.buffer_seconds = clamp_buffer(finite(seconds, "bufferSeconds")?);
    }
    if let Some(sensitivity) = patch.sensitivity {
        next.sensitivity = sensitivity;
    }
    if let Some(device) = patch.output_device {
        next.output_device = match device {
            None => None,
            Some(device) if device.is_empty() => {
                return Err(CommandError::invalid(
                    "The output device needs a name; use null for the system default",
                ));
            }
            Some(device) => Some(device),
        };
    }
    if let Some(speed) = patch.replay_speed {
        next.replay_speed = clamp_range(finite(speed, "replaySpeed")?, REPLAY_SPEED);
    }
    if let Some(speed) = patch.catch_up_speed {
        next.catch_up_speed = clamp_range(finite(speed, "catchUpSpeed")?, CATCH_UP_SPEED);
    }
    if let Some(duck) = patch.duck {
        if let Some(enabled) = duck.enabled {
            next.duck.enabled = enabled;
        }
        if let Some(level) = duck.level {
            next.duck.level = clamp_range(finite(level, "duck.level")?, DUCK_LEVEL);
        }
    }
    if let Some(hotkeys) = patch.hotkeys {
        for (action, change) in [
            (AppAction::Replay, hotkeys.replay),
            (AppAction::Stop, hotkeys.stop),
            (AppAction::CatchUp, hotkeys.catch_up),
        ] {
            let Some(change) = change else { continue };
            let accel = match change {
                None => None,
                Some(accel) => {
                    if let Err(reason) = parse_accel(&accel) {
                        return Err(CommandError::invalid(format!(
                            "The {} hotkey \"{accel}\" can't be used: {reason}",
                            action_label(action)
                        )));
                    }
                    Some(accel.trim().to_string())
                }
            };
            next.hotkeys.set(action, accel);
        }
        check_conflicts(&next.hotkeys)?;
    }
    if let Some(autostart) = patch.autostart {
        next.autostart = autostart;
    }
    if let Some(done) = patch.first_run_done {
        next.first_run_done = done;
    }
    Ok(next)
}

/// Two actions on one hotkey is an error, whichever way the combination is spelled.
fn check_conflicts(hotkeys: &HotkeyBindings) -> Result<(), CommandError> {
    let bound: Vec<(AppAction, &str, Shortcut)> = HOTKEY_ACTIONS
        .iter()
        .filter_map(|&action| {
            let accel = hotkeys.accel(action)?;
            Some((action, accel, parse_accel(accel).ok()?))
        })
        .collect();
    for (index, (first, accel, shortcut)) in bound.iter().enumerate() {
        if let Some((second, _, _)) = bound[index + 1..].iter().find(|(_, _, other)| other == shortcut) {
            return Err(CommandError::conflict(format!(
                "{} and {} can't use the same hotkey ({accel})",
                action_label(*first),
                action_label(*second)
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{CommandErrorKind, Duck, Sensitivity, Target};
    use serde_json::json;

    fn patch(value: Value) -> SettingsPatch {
        serde_json::from_value(value).expect("test patch parses")
    }

    fn apply(value: Value) -> Result<Settings, CommandError> {
        apply_patch(&Settings::default(), patch(value))
    }

    fn hotkeys(replay: Option<&str>, stop: Option<&str>, catch_up: Option<&str>) -> HotkeyBindings {
        HotkeyBindings {
            replay: replay.map(String::from),
            stop: stop.map(String::from),
            catch_up: catch_up.map(String::from),
        }
    }

    #[test]
    fn defaults_match_the_contract() {
        assert_eq!(
            serde_json::to_value(Settings::default()).unwrap(),
            json!({
                "version": 1,
                "target": null,
                "bufferSeconds": 30,
                "sensitivity": "normal",
                "outputDevice": null,
                "replaySpeed": 1.0,
                "catchUpSpeed": 1.5,
                "duck": { "enabled": true, "level": 0.2 },
                "hotkeys": { "replay": "Ctrl+Alt+Space", "stop": "Ctrl+Alt+Backspace", "catchUp": "Ctrl+Alt+Enter" },
                "autostart": false,
                "firstRunDone": false
            })
        );
    }

    #[test]
    fn default_hotkeys_parse_and_differ() {
        let defaults = HotkeyBindings::default();
        let parsed: Vec<_> = HOTKEY_ACTIONS
            .iter()
            .map(|a| parse_accel(defaults.accel(*a).unwrap()).unwrap())
            .collect();
        assert_ne!(parsed[0], parsed[1]);
        assert_ne!(parsed[0], parsed[2]);
        assert_ne!(parsed[1], parsed[2]);
    }

    #[test]
    fn accel_is_only_defined_for_the_hotkey_actions() {
        let defaults = HotkeyBindings::default();
        assert_eq!(defaults.accel(AppAction::Replay), Some("Ctrl+Alt+Space"));
        assert_eq!(defaults.accel(AppAction::Stop), Some("Ctrl+Alt+Backspace"));
        assert_eq!(defaults.accel(AppAction::CatchUp), Some("Ctrl+Alt+Enter"));
        assert_eq!(defaults.accel(AppAction::Quit), None);
        assert_eq!(defaults.accel(AppAction::TogglePause), None);
    }

    #[test]
    fn accelerators_parse_loosely_but_not_garbage() {
        assert_eq!(parse_accel("Ctrl+Alt+Space"), parse_accel("  control + ALT + space "));
        assert_eq!(parse_accel("Ctrl+Alt+Enter"), parse_accel("Alt+Ctrl+Enter"));
        assert!(parse_accel("Ctrl+Alt+Frobnicate").is_err());
        assert!(parse_accel("Ctrl+Alt").is_err());
        assert!(parse_accel("").is_err());
        assert!(parse_accel("Ctrl++Space").is_err());
    }

    // sanitize

    #[test]
    fn nothing_stored_gives_the_defaults() {
        assert_eq!(sanitize(&Value::Null), Settings::default());
        assert_eq!(sanitize(&json!({})), Settings::default());
        assert_eq!(sanitize(&json!("garbage")), Settings::default());
        assert_eq!(sanitize(&json!([1, 2])), Settings::default());
    }

    #[test]
    fn a_full_stored_object_is_read_back() {
        let stored = json!({
            "version": 1,
            "target": { "exePath": "C:\\Discord.exe", "name": "Discord" },
            "bufferSeconds": 45,
            "sensitivity": "high",
            "outputDevice": "Headphones",
            "replaySpeed": 1.25,
            "catchUpSpeed": 1.75,
            "duck": { "enabled": false, "level": 0.5 },
            "hotkeys": { "replay": "Ctrl+Shift+R", "stop": null, "catchUp": "Ctrl+Shift+C" },
            "autostart": true,
            "firstRunDone": true
        });
        let settings = sanitize(&stored);
        assert_eq!(
            settings,
            Settings {
                version: 1,
                target: Some(Target {
                    exe_path: Some("C:\\Discord.exe".into()),
                    name: "Discord".into()
                }),
                buffer_seconds: 45,
                sensitivity: Sensitivity::High,
                output_device: Some("Headphones".into()),
                replay_speed: 1.25,
                catch_up_speed: 1.75,
                duck: Duck {
                    enabled: false,
                    level: 0.5
                },
                hotkeys: hotkeys(Some("Ctrl+Shift+R"), None, Some("Ctrl+Shift+C")),
                autostart: true,
                first_run_done: true,
            }
        );
        // And it survives a round trip through JSON.
        assert_eq!(sanitize(&serde_json::to_value(&settings).unwrap()), settings);
    }

    #[test]
    fn each_bad_field_falls_back_on_its_own() {
        let settings = sanitize(&json!({
            "target": 7,
            "bufferSeconds": "lots",
            "sensitivity": "loud",
            "outputDevice": 3,
            "replaySpeed": null,
            "catchUpSpeed": 1.8,
            "duck": { "enabled": "yes", "level": 0.4 },
            "hotkeys": { "replay": 5, "stop": "Ctrl+Alt+Frobnicate", "catchUp": "Ctrl+Shift+C" },
            "autostart": 1,
            "firstRunDone": true
        }));
        let defaults = Settings::default();
        assert_eq!(settings.target, None);
        assert_eq!(settings.buffer_seconds, defaults.buffer_seconds);
        assert_eq!(settings.sensitivity, defaults.sensitivity);
        assert_eq!(settings.output_device, None);
        assert_eq!(settings.replay_speed, defaults.replay_speed);
        assert_eq!(
            settings.catch_up_speed, 1.8,
            "the good field next to the bad ones is kept"
        );
        assert_eq!(
            settings.duck,
            Duck {
                enabled: true,
                level: 0.4
            }
        );
        assert_eq!(
            settings.hotkeys,
            hotkeys(Some("Ctrl+Alt+Space"), Some("Ctrl+Alt+Backspace"), Some("Ctrl+Shift+C"))
        );
        assert!(!settings.autostart);
        assert!(settings.first_run_done);
    }

    #[test]
    fn stored_numbers_are_clamped() {
        let low = sanitize(&json!({
            "bufferSeconds": 1, "replaySpeed": 0.1, "catchUpSpeed": 0.5, "duck": { "level": 0.0 }
        }));
        assert_eq!(low.buffer_seconds, 10);
        assert_eq!(low.replay_speed, 0.75);
        assert_eq!(low.catch_up_speed, 1.25);
        assert_eq!(low.duck.level, 0.05);
        let high = sanitize(&json!({
            "bufferSeconds": 9000, "replaySpeed": 4, "catchUpSpeed": 9, "duck": { "level": 3 }
        }));
        assert_eq!(high.buffer_seconds, 120);
        assert_eq!(high.replay_speed, 1.5);
        assert_eq!(high.catch_up_speed, 2.0);
        assert_eq!(high.duck.level, 1.0);
    }

    #[test]
    fn a_fractional_buffer_is_rounded() {
        assert_eq!(sanitize(&json!({ "bufferSeconds": 44.6 })).buffer_seconds, 45);
        assert_eq!(sanitize(&json!({ "bufferSeconds": -5 })).buffer_seconds, 10);
    }

    #[test]
    fn a_target_needs_a_name() {
        assert_eq!(sanitize(&json!({ "target": { "exePath": "x.exe" } })).target, None);
        assert_eq!(sanitize(&json!({ "target": { "name": "  " } })).target, None);
        assert_eq!(
            sanitize(&json!({ "target": { "name": " Teams " } })).target,
            Some(Target {
                exe_path: None,
                name: "Teams".into()
            })
        );
        // A path of the wrong type is dropped, the app is still remembered by name.
        assert_eq!(
            sanitize(&json!({ "target": { "exePath": 5, "name": "Teams" } })).target,
            Some(Target {
                exe_path: None,
                name: "Teams".into()
            })
        );
    }

    #[test]
    fn the_first_run_flag_is_read_from_the_stored_object() {
        assert!(!sanitize(&json!({})).first_run_done);
        assert!(!sanitize(&json!({ "firstRunDone": false })).first_run_done);
        let done = sanitize(&json!({ "firstRunDone": true, "bufferSeconds": 30 }));
        assert!(done.first_run_done);
    }

    #[test]
    fn a_broken_first_run_flag_counts_as_a_first_run() {
        for broken in [json!("yes"), json!(1), json!(null), json!([true])] {
            assert!(!sanitize(&json!({ "firstRunDone": broken })).first_run_done);
        }
        assert!(!sanitize(&json!("garbage")).first_run_done);
        assert!(!sanitize(&Value::Null).first_run_done);
    }

    #[test]
    fn an_empty_output_device_means_the_default() {
        assert_eq!(sanitize(&json!({ "outputDevice": "" })).output_device, None);
    }

    #[test]
    fn a_null_hotkey_stays_unbound() {
        let settings = sanitize(&json!({ "hotkeys": { "replay": null } }));
        assert_eq!(settings.hotkeys.replay, None);
        assert_eq!(settings.hotkeys.stop.as_deref(), Some("Ctrl+Alt+Backspace"));
    }

    #[test]
    fn a_stored_duplicate_hotkey_is_unbound_for_the_later_action() {
        let settings = sanitize(&json!({
            "hotkeys": { "replay": "Ctrl+Shift+R", "stop": "shift + control + r", "catchUp": "Ctrl+Alt+Enter" }
        }));
        assert_eq!(
            settings.hotkeys,
            hotkeys(Some("Ctrl+Shift+R"), None, Some("Ctrl+Alt+Enter"))
        );
    }

    #[test]
    fn a_default_that_clashes_with_a_stored_hotkey_is_unbound() {
        // Replay was moved onto catch-up's default and catch-up is missing, so it would fall back onto the same keys.
        let settings = sanitize(&json!({ "hotkeys": { "replay": "Ctrl+Alt+Enter" } }));
        assert_eq!(
            settings.hotkeys,
            hotkeys(Some("Ctrl+Alt+Enter"), Some("Ctrl+Alt+Backspace"), None)
        );
    }

    // apply_patch

    #[test]
    fn an_empty_patch_changes_nothing() {
        assert_eq!(apply(json!({})).unwrap(), Settings::default());
    }

    #[test]
    fn a_patch_changes_only_the_named_fields() {
        let settings = apply(json!({
            "bufferSeconds": 60,
            "sensitivity": "low",
            "replaySpeed": 1.25,
            "catchUpSpeed": 2,
            "autostart": true,
            "firstRunDone": true
        }))
        .unwrap();
        let defaults = Settings::default();
        assert_eq!(settings.buffer_seconds, 60);
        assert_eq!(settings.sensitivity, Sensitivity::Low);
        assert_eq!(settings.replay_speed, 1.25);
        assert_eq!(settings.catch_up_speed, 2.0);
        assert!(settings.autostart);
        assert!(settings.first_run_done);
        assert_eq!(settings.duck, defaults.duck);
        assert_eq!(settings.hotkeys, defaults.hotkeys);
        assert_eq!(settings.target, None);
    }

    #[test]
    fn nested_patches_merge_into_the_nested_object() {
        let settings = apply(json!({ "duck": { "level": 0.5 }, "hotkeys": { "stop": "Ctrl+Alt+S" } })).unwrap();
        assert_eq!(
            settings.duck,
            Duck {
                enabled: true,
                level: 0.5
            }
        );
        assert_eq!(
            settings.hotkeys,
            hotkeys(Some("Ctrl+Alt+Space"), Some("Ctrl+Alt+S"), Some("Ctrl+Alt+Enter"))
        );
        let settings = apply(json!({ "duck": { "enabled": false } })).unwrap();
        assert_eq!(
            settings.duck,
            Duck {
                enabled: false,
                level: 0.2
            }
        );
    }

    #[test]
    fn the_window_may_send_duck_and_hotkeys_as_whole_objects() {
        let settings = apply(json!({
            "duck": { "enabled": false, "level": 0.4 },
            "hotkeys": { "replay": "Ctrl+Shift+R", "stop": null, "catchUp": "Ctrl+Shift+C" }
        }))
        .unwrap();
        assert_eq!(
            settings.duck,
            Duck {
                enabled: false,
                level: 0.4
            }
        );
        assert_eq!(
            settings.hotkeys,
            hotkeys(Some("Ctrl+Shift+R"), None, Some("Ctrl+Shift+C"))
        );
    }

    #[test]
    fn patch_numbers_are_clamped() {
        let settings = apply(json!({
            "bufferSeconds": 5000, "replaySpeed": 0.1, "catchUpSpeed": 10, "duck": { "level": 0 }
        }))
        .unwrap();
        assert_eq!(settings.buffer_seconds, 120);
        assert_eq!(settings.replay_speed, 0.75);
        assert_eq!(settings.catch_up_speed, 2.0);
        assert_eq!(settings.duck.level, 0.05);
    }

    #[test]
    fn null_clears_the_fields_where_null_means_something() {
        let current = Settings {
            target: Some(Target {
                exe_path: None,
                name: "Discord".into(),
            }),
            output_device: Some("Headphones".into()),
            ..Settings::default()
        };
        let cleared = apply_patch(
            &current,
            patch(json!({ "target": null, "outputDevice": null, "hotkeys": { "replay": null } })),
        )
        .unwrap();
        assert_eq!(cleared.target, None);
        assert_eq!(cleared.output_device, None);
        assert_eq!(cleared.hotkeys.replay, None);
        assert_eq!(cleared.hotkeys.stop, current.hotkeys.stop);
        // Absent is not the same as null.
        let untouched = apply_patch(&current, patch(json!({ "bufferSeconds": 20 }))).unwrap();
        assert_eq!(untouched.target, current.target);
        assert_eq!(untouched.output_device, current.output_device);
    }

    #[test]
    fn a_target_and_an_output_device_can_be_set() {
        let settings = apply(json!({
            "target": { "exePath": "C:\\Teams.exe", "name": " Teams " },
            "outputDevice": "Headphones"
        }))
        .unwrap();
        assert_eq!(
            settings.target,
            Some(Target {
                exe_path: Some("C:\\Teams.exe".into()),
                name: "Teams".into()
            })
        );
        assert_eq!(settings.output_device.as_deref(), Some("Headphones"));
        let by_name = apply(json!({ "target": { "exePath": null, "name": "speech_8s" } })).unwrap();
        assert_eq!(by_name.target.unwrap().exe_path, None);
    }

    #[test]
    fn unusable_values_are_invalid() {
        for bad in [
            json!({ "target": { "name": "" } }),
            json!({ "target": { "name": "   " } }),
            json!({ "outputDevice": "" }),
        ] {
            let err = apply(bad.clone()).expect_err(&bad.to_string());
            assert_eq!(err.kind, CommandErrorKind::Invalid, "{bad}");
        }
    }

    #[test]
    fn values_of_the_wrong_type_do_not_even_parse_as_a_patch() {
        for bad in [
            json!({ "target": { "exePath": "x.exe" } }),
            json!({ "bufferSeconds": "lots" }),
            json!({ "sensitivity": "loud" }),
            json!({ "duck": { "level": "half" } }),
            json!({ "hotkeys": "Ctrl+Alt+Space" }),
            json!({ "autostart": "yes" }),
        ] {
            assert!(serde_json::from_value::<SettingsPatch>(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_unparseable_hotkey_is_invalid_and_names_the_action() {
        let err = apply(json!({ "hotkeys": { "catchUp": "Ctrl+Alt+Frobnicate" } })).unwrap_err();
        assert_eq!(err.kind, CommandErrorKind::Invalid);
        assert!(err.message.contains("Catch-up"), "{}", err.message);
        assert!(err.message.contains("Ctrl+Alt+Frobnicate"), "{}", err.message);
        assert_eq!(
            apply(json!({ "hotkeys": { "replay": "" } })).unwrap_err().kind,
            CommandErrorKind::Invalid
        );
        assert_eq!(
            apply(json!({ "hotkeys": { "replay": "Ctrl+Alt" } })).unwrap_err().kind,
            CommandErrorKind::Invalid
        );
    }

    #[test]
    fn two_actions_on_one_hotkey_conflict() {
        let err = apply(json!({ "hotkeys": { "stop": "Ctrl+Alt+Space" } })).unwrap_err();
        assert_eq!(err.kind, CommandErrorKind::Conflict);
        assert!(err.message.contains("Replay"), "{}", err.message);
        assert!(err.message.contains("Stop"), "{}", err.message);
    }

    #[test]
    fn a_conflict_is_found_whatever_the_spelling() {
        let err = apply(json!({ "hotkeys": { "catchUp": "alt + control + SPACE" } })).unwrap_err();
        assert_eq!(err.kind, CommandErrorKind::Conflict);
    }

    #[test]
    fn swapping_two_hotkeys_is_not_a_conflict() {
        let settings = apply(json!({
            "hotkeys": { "replay": "Ctrl+Alt+Backspace", "stop": "Ctrl+Alt+Space" }
        }))
        .unwrap();
        assert_eq!(
            settings.hotkeys,
            hotkeys(
                Some("Ctrl+Alt+Backspace"),
                Some("Ctrl+Alt+Space"),
                Some("Ctrl+Alt+Enter")
            )
        );
    }

    #[test]
    fn freeing_a_hotkey_and_reusing_it_in_one_patch_works() {
        let settings = apply(json!({ "hotkeys": { "replay": null, "stop": "Ctrl+Alt+Space" } })).unwrap();
        assert_eq!(settings.hotkeys.replay, None);
        assert_eq!(settings.hotkeys.stop.as_deref(), Some("Ctrl+Alt+Space"));
    }

    #[test]
    fn rebinding_an_action_to_its_own_hotkey_is_fine() {
        let settings = apply(json!({ "hotkeys": { "replay": "ctrl+alt+space" } })).unwrap();
        assert_eq!(settings.hotkeys.replay.as_deref(), Some("ctrl+alt+space"));
    }

    #[test]
    fn unbound_actions_never_conflict() {
        let settings = apply(json!({ "hotkeys": { "replay": null, "stop": null, "catchUp": null } })).unwrap();
        assert_eq!(settings.hotkeys, hotkeys(None, None, None));
    }

    #[test]
    fn a_failed_patch_leaves_the_input_alone() {
        let current = Settings::default();
        let _ = apply_patch(
            &current,
            patch(json!({ "bufferSeconds": 99, "hotkeys": { "stop": "Ctrl+Alt+Space" } })),
        );
        assert_eq!(current, Settings::default());
    }

    #[test]
    fn unknown_fields_and_version_in_a_patch_are_ignored() {
        let settings = apply(json!({ "version": 7, "somethingNew": true, "bufferSeconds": 40 })).unwrap();
        assert_eq!(settings.version, 1);
        assert_eq!(settings.buffer_seconds, 40);
    }

    #[test]
    fn a_whole_settings_object_works_as_a_patch() {
        let wanted = Settings {
            buffer_seconds: 90,
            sensitivity: Sensitivity::Low,
            first_run_done: true,
            ..Settings::default()
        };
        let applied = apply(serde_json::to_value(&wanted).unwrap()).unwrap();
        assert_eq!(applied, wanted);
    }
}
