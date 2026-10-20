//! The tray icon and its menu. [`build_model`] is a pure function from the app status to what the tray shows, so it can
//! be tested without a display; the rest turns a [`TrayModel`] into the real Tauri tray.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::actions::{self, AppAction};

/// The window title and the start of the tooltip.
const APP_NAME: &str = "Repeat that?!";

/// Which of the three tray icons to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayIconKind {
    /// No app picked, waiting for the picked app, or an error.
    Idle,
    Capturing,
    Paused,
}

/// What capture is doing, as far as the tray cares.
// Until the audio engine feeds the tray only `NoTarget` and `Paused` happen; the other states are tested.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureView {
    NoTarget,
    /// The picked app (by name) isn't running.
    Waiting(String),
    Capturing(String),
    Paused,
    Error(String),
}

// Constructed once the audio engine feeds the tray; tested until then.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaybackView {
    #[default]
    Idle,
    Replaying,
    CatchingUp,
}

/// The slice of the app status the tray shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayStatus {
    pub capture: CaptureView,
    pub playback: PlaybackView,
    /// Speech in the buffer.
    pub buffered: Duration,
    /// Distance from the play cursor to live, while a replay runs.
    pub behind: Option<Duration>,
}

impl Default for TrayStatus {
    fn default() -> Self {
        TrayStatus {
            capture: CaptureView::NoTarget,
            playback: PlaybackView::Idle,
            buffered: Duration::ZERO,
            behind: None,
        }
    }
}

impl TrayStatus {
    pub fn is_paused(&self) -> bool {
        self.capture == CaptureView::Paused
    }

    /// Pausing wipes the buffer and stops playback. There is no engine to resume to yet, so resuming goes back to "no
    /// app selected".
    pub fn set_paused(&mut self, paused: bool) {
        if paused {
            *self = TrayStatus {
                capture: CaptureView::Paused,
                ..TrayStatus::default()
            };
        } else if self.is_paused() {
            self.capture = CaptureView::NoTarget;
        }
    }

    pub fn icon(&self) -> TrayIconKind {
        match self.capture {
            CaptureView::Capturing(_) => TrayIconKind::Capturing,
            CaptureView::Paused => TrayIconKind::Paused,
            CaptureView::NoTarget | CaptureView::Waiting(_) | CaptureView::Error(_) => TrayIconKind::Idle,
        }
    }

    /// One line for the first menu item and the tooltip, e.g. "Capturing Discord · 24 s of speech".
    pub fn status_line(&self) -> String {
        let playing = match self.playback {
            PlaybackView::Idle => None,
            PlaybackView::Replaying => Some("Replaying"),
            PlaybackView::CatchingUp => Some("Catching up"),
        };
        if let Some(verb) = playing {
            return match self.behind {
                Some(behind) => format!("{verb} · {} s behind live", whole_seconds(behind)),
                None => verb.to_string(),
            };
        }
        match &self.capture {
            CaptureView::NoTarget => "No app selected".to_string(),
            CaptureView::Waiting(name) => format!("Waiting for {name}"),
            CaptureView::Capturing(name) => format!("Capturing {name} · {} s of speech", whole_seconds(self.buffered)),
            CaptureView::Paused => "Paused".to_string(),
            CaptureView::Error(message) => message.clone(),
        }
    }
}

fn whole_seconds(duration: Duration) -> u64 {
    (duration.as_millis() as u64 + 500) / 1000
}

/// The hotkeys shown next to the menu items; `None` = not bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyLabels {
    pub replay: Option<String>,
    pub catch_up: Option<String>,
    pub stop: Option<String>,
}

impl Default for HotkeyLabels {
    fn default() -> Self {
        HotkeyLabels {
            replay: Some("Ctrl+Alt+Space".into()),
            catch_up: Some("Ctrl+Alt+Enter".into()),
            stop: Some("Ctrl+Alt+Backspace".into()),
        }
    }
}

pub const STATUS_ID: &str = "status";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItemModel {
    /// For items that run an action this is [`AppAction::id`](crate::actions::AppAction::id).
    pub id: &'static str,
    pub label: String,
    pub enabled: bool,
    /// `Some` makes it a checkbox.
    pub checked: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Item(MenuItemModel),
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayModel {
    pub icon: TrayIconKind,
    pub tooltip: String,
    pub entries: Vec<MenuEntry>,
}

/// Everything the tray shows for `status`: icon, tooltip and menu.
pub fn build_model(status: &TrayStatus, hotkeys: &HotkeyLabels) -> TrayModel {
    let status_line = status.status_line();
    let action = |action: AppAction, label: &str, hotkey: Option<&str>| {
        let label = match hotkey {
            Some(hotkey) => format!("{label} ({hotkey})"),
            None => label.to_string(),
        };
        MenuEntry::Item(MenuItemModel {
            id: action.id(),
            label,
            enabled: true,
            checked: None,
        })
    };
    let entries = vec![
        MenuEntry::Item(MenuItemModel {
            id: STATUS_ID,
            label: status_line.clone(),
            enabled: false,
            checked: None,
        }),
        action(AppAction::Replay, "Replay", hotkeys.replay.as_deref()),
        action(AppAction::CatchUp, "Catch-up", hotkeys.catch_up.as_deref()),
        action(AppAction::Stop, "Stop", hotkeys.stop.as_deref()),
        MenuEntry::Separator,
        MenuEntry::Item(MenuItemModel {
            id: AppAction::TogglePause.id(),
            label: "Pause capture".into(),
            enabled: true,
            checked: Some(status.is_paused()),
        }),
        action(AppAction::OpenSettings, "Settings…", None),
        MenuEntry::Separator,
        action(AppAction::Quit, "Quit", None),
    ];
    TrayModel {
        icon: status.icon(),
        tooltip: format!("{APP_NAME} - {status_line}"),
        entries,
    }
}

const TRAY_ID: &str = "main";

/// The tray menu, kept to update its items in place.
struct TrayMenu<R: Runtime>(Menu<R>);

/// The shared status the tray shows. Locked briefly; never held across a call into the UI toolkit.
pub type SharedStatus = Mutex<TrayStatus>;

/// Locks the status, ignoring poisoning: a panic elsewhere must not take the tray down with it.
pub fn lock_status(status: &SharedStatus) -> MutexGuard<'_, TrayStatus> {
    status.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Creates the tray icon and menu. Needs [`SharedStatus`] in the managed state.
pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let model = current_model(app);
    let menu = build_menu(app, &model)?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon_image(model.icon)?)
        .tooltip(&model.tooltip)
        .menu(&menu)
        // Windows: a left click opens the settings, the menu is on the right click. Linux (appindicator) doesn't send
        // click events and only has the menu.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if let Some(action) = AppAction::from_id(event.id().as_ref()) {
                actions::dispatch(action, app);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                actions::dispatch(AppAction::OpenSettings, tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(TrayMenu(menu));
    Ok(())
}

/// Shows the current [`SharedStatus`] in the tray: icon, tooltip and menu items.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let model = current_model(app);
    if let Some(menu) = app.try_state::<TrayMenu<R>>() {
        for entry in &model.entries {
            let MenuEntry::Item(item) = entry else { continue };
            // Best effort: a menu item that can't be updated keeps its old text.
            match menu.0.get(item.id) {
                Some(MenuItemKind::MenuItem(menu_item)) => {
                    let _ = menu_item.set_text(&item.label);
                    let _ = menu_item.set_enabled(item.enabled);
                }
                Some(MenuItemKind::Check(check)) => {
                    let _ = check.set_text(&item.label);
                    let _ = check.set_enabled(item.enabled);
                    let _ = check.set_checked(item.checked.unwrap_or(false));
                }
                _ => {}
            }
        }
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Ok(image) = icon_image(model.icon) {
            let _ = tray.set_icon(Some(image));
        }
        let _ = tray.set_tooltip(Some(model.tooltip));
    }
}

fn current_model<R: Runtime>(app: &AppHandle<R>) -> TrayModel {
    let status = app.state::<SharedStatus>();
    // Hotkey labels come from the settings once those exist.
    build_model(&lock_status(&status), &HotkeyLabels::default())
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, model: &TrayModel) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for entry in &model.entries {
        match entry {
            MenuEntry::Separator => menu.append(&PredefinedMenuItem::separator(app)?)?,
            MenuEntry::Item(item) => match item.checked {
                Some(checked) => menu.append(&CheckMenuItem::with_id(
                    app,
                    item.id,
                    &item.label,
                    item.enabled,
                    checked,
                    None::<&str>,
                )?)?,
                None => menu.append(&MenuItem::with_id(
                    app,
                    item.id,
                    &item.label,
                    item.enabled,
                    None::<&str>,
                )?)?,
            },
        }
    }
    Ok(menu)
}

fn icon_image(kind: TrayIconKind) -> tauri::Result<Image<'static>> {
    let png: &[u8] = match kind {
        TrayIconKind::Idle => include_bytes!("../icons/tray/idle.png"),
        TrayIconKind::Capturing => include_bytes!("../icons/tray/capturing.png"),
        TrayIconKind::Paused => include_bytes!("../icons/tray/paused.png"),
    };
    Image::from_bytes(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    fn capturing(name: &str, buffered: u64) -> TrayStatus {
        TrayStatus {
            capture: CaptureView::Capturing(name.into()),
            buffered: secs(buffered),
            ..TrayStatus::default()
        }
    }

    fn items(model: &TrayModel) -> Vec<&MenuItemModel> {
        model
            .entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Item(item) => Some(item),
                MenuEntry::Separator => None,
            })
            .collect()
    }

    fn item<'a>(model: &'a TrayModel, id: &str) -> &'a MenuItemModel {
        items(model)
            .into_iter()
            .find(|i| i.id == id)
            .unwrap_or_else(|| panic!("no menu item {id}"))
    }

    #[test]
    fn status_lines() {
        let line = |capture, playback, buffered, behind: Option<u64>| {
            TrayStatus {
                capture,
                playback,
                buffered: secs(buffered),
                behind: behind.map(secs),
            }
            .status_line()
        };
        use CaptureView::*;
        use PlaybackView::*;
        assert_eq!(line(NoTarget, Idle, 0, None), "No app selected");
        assert_eq!(line(Waiting("Discord".into()), Idle, 0, None), "Waiting for Discord");
        assert_eq!(
            line(Capturing("Discord".into()), Idle, 24, None),
            "Capturing Discord · 24 s of speech"
        );
        assert_eq!(line(Paused, Idle, 0, None), "Paused");
        assert_eq!(
            line(Error("Audio service is not running".into()), Idle, 0, None),
            "Audio service is not running"
        );
        assert_eq!(
            line(Capturing("Discord".into()), Replaying, 24, Some(3)),
            "Replaying · 3 s behind live"
        );
        assert_eq!(
            line(Capturing("Discord".into()), CatchingUp, 24, Some(12)),
            "Catching up · 12 s behind live"
        );
        assert_eq!(line(Capturing("Discord".into()), Replaying, 24, None), "Replaying");
    }

    #[test]
    fn behind_live_is_rounded_to_whole_seconds() {
        let status = TrayStatus {
            playback: PlaybackView::Replaying,
            behind: Some(Duration::from_millis(2600)),
            ..capturing("Discord", 10)
        };
        assert_eq!(status.status_line(), "Replaying · 3 s behind live");
        let status = TrayStatus {
            behind: Some(Duration::from_millis(400)),
            ..status
        };
        assert_eq!(status.status_line(), "Replaying · 0 s behind live");
    }

    #[test]
    fn icon_follows_the_capture_state() {
        use CaptureView::*;
        let icon = |capture| {
            TrayStatus {
                capture,
                ..TrayStatus::default()
            }
            .icon()
        };
        assert_eq!(icon(NoTarget), TrayIconKind::Idle);
        assert_eq!(icon(Waiting("Discord".into())), TrayIconKind::Idle);
        assert_eq!(icon(Error("boom".into())), TrayIconKind::Idle);
        assert_eq!(icon(Capturing("Discord".into())), TrayIconKind::Capturing);
        assert_eq!(icon(Paused), TrayIconKind::Paused);
    }

    #[test]
    fn icon_stays_while_replaying() {
        let status = TrayStatus {
            playback: PlaybackView::Replaying,
            ..capturing("Discord", 5)
        };
        assert_eq!(status.icon(), TrayIconKind::Capturing);
    }

    #[test]
    fn tooltip_is_the_app_name_and_the_status_line() {
        let model = build_model(&capturing("Discord", 24), &HotkeyLabels::default());
        assert_eq!(model.tooltip, "Repeat that?! - Capturing Discord · 24 s of speech");
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default());
        assert_eq!(model.tooltip, "Repeat that?! - No app selected");
    }

    #[test]
    fn menu_layout() {
        let model = build_model(&capturing("Discord", 24), &HotkeyLabels::default());
        let layout: Vec<String> = model
            .entries
            .iter()
            .map(|e| match e {
                MenuEntry::Item(i) => format!("{}: {}", i.id, i.label),
                MenuEntry::Separator => "---".into(),
            })
            .collect();
        assert_eq!(
            layout,
            [
                "status: Capturing Discord · 24 s of speech",
                "replay: Replay (Ctrl+Alt+Space)",
                "catch_up: Catch-up (Ctrl+Alt+Enter)",
                "stop: Stop (Ctrl+Alt+Backspace)",
                "---",
                "toggle_pause: Pause capture",
                "open_settings: Settings…",
                "---",
                "quit: Quit",
            ]
        );
    }

    #[test]
    fn status_line_item_is_disabled_and_the_rest_are_enabled() {
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default());
        for i in items(&model) {
            assert_eq!(i.enabled, i.id != STATUS_ID, "{}", i.id);
        }
    }

    #[test]
    fn pause_item_is_a_checkbox_that_follows_the_state() {
        let hotkeys = HotkeyLabels::default();
        let running = build_model(&capturing("Discord", 1), &hotkeys);
        assert_eq!(item(&running, AppAction::TogglePause.id()).checked, Some(false));
        let mut paused = capturing("Discord", 1);
        paused.set_paused(true);
        let model = build_model(&paused, &hotkeys);
        assert_eq!(item(&model, AppAction::TogglePause.id()).checked, Some(true));
        assert_eq!(model.icon, TrayIconKind::Paused);
        assert_eq!(item(&model, STATUS_ID).label, "Paused");
        for i in items(&model)
            .into_iter()
            .filter(|i| i.id != AppAction::TogglePause.id())
        {
            assert_eq!(i.checked, None, "{}", i.id);
        }
    }

    #[test]
    fn unbound_hotkeys_show_the_bare_label() {
        let hotkeys = HotkeyLabels {
            replay: None,
            catch_up: Some("Ctrl+Alt+C".into()),
            stop: None,
        };
        let model = build_model(&TrayStatus::default(), &hotkeys);
        assert_eq!(item(&model, AppAction::Replay.id()).label, "Replay");
        assert_eq!(item(&model, AppAction::CatchUp.id()).label, "Catch-up (Ctrl+Alt+C)");
        assert_eq!(item(&model, AppAction::Stop.id()).label, "Stop");
    }

    #[test]
    fn every_menu_item_except_the_status_line_maps_to_an_action() {
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default());
        for i in items(&model).into_iter().filter(|i| i.id != STATUS_ID) {
            assert!(AppAction::from_id(i.id).is_some(), "{}", i.id);
        }
    }

    #[test]
    fn pausing_wipes_the_status_and_resuming_goes_back_to_no_target() {
        let mut status = TrayStatus {
            playback: PlaybackView::Replaying,
            behind: Some(secs(2)),
            ..capturing("Discord", 30)
        };
        status.set_paused(true);
        assert!(status.is_paused());
        assert_eq!(
            status,
            TrayStatus {
                capture: CaptureView::Paused,
                ..TrayStatus::default()
            }
        );
        status.set_paused(false);
        assert!(!status.is_paused());
        assert_eq!(status, TrayStatus::default());
    }

    #[test]
    fn resuming_when_not_paused_changes_nothing() {
        let mut status = capturing("Discord", 30);
        status.set_paused(false);
        assert_eq!(status, capturing("Discord", 30));
    }
}
