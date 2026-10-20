//! The tray icon and its menu. [`build_model`] is a pure function from the app status to what the tray shows, so it can
//! be tested without a display; the rest turns a [`TrayModel`] into the real Tauri tray.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::actions::{self, AppAction};

/// The window title and the start of the tooltip.
const APP_NAME: &str = "Repeat That";

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

/// The hotkeys shown next to the menu items; `None` = not bound. Nothing is bound until the hotkey registration says
/// so, so the default shows no hotkeys rather than ones that may not work.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HotkeyLabels {
    pub replay: Option<String>,
    pub catch_up: Option<String>,
    pub stop: Option<String>,
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

/// Windows: a left click on the icon opens the settings and the menu is on the right click. On macOS a click on a
/// menu bar item opens its menu (Settings is in it), and Linux's appindicator sends no click events, only the menu.
const LEFT_CLICK_OPENS_SETTINGS: bool = cfg!(windows);

/// The three tray icons, decoded once at startup. Decoding a PNG on every status change (about once a second while
/// capturing) costs more than it is worth.
struct TrayIcons {
    idle: Image<'static>,
    capturing: Image<'static>,
    paused: Image<'static>,
}

impl TrayIcons {
    fn load() -> tauri::Result<Self> {
        Ok(TrayIcons {
            idle: Image::from_bytes(include_bytes!("../icons/tray/idle.png"))?,
            capturing: Image::from_bytes(include_bytes!("../icons/tray/capturing.png"))?,
            paused: Image::from_bytes(include_bytes!("../icons/tray/paused.png"))?,
        })
    }

    fn image(&self, kind: TrayIconKind) -> Image<'static> {
        match kind {
            TrayIconKind::Idle => &self.idle,
            TrayIconKind::Capturing => &self.capturing,
            TrayIconKind::Paused => &self.paused,
        }
        .clone()
    }
}

/// The real tray with what [`refresh`] needs to update it, and what it shows now (`applied`).
struct TrayView<R: Runtime> {
    menu: Menu<R>,
    icons: TrayIcons,
    applied: Mutex<TrayModel>,
}

/// What can be changed on the tray. Each call says whether it worked. The real one is [`LiveTray`]; the tests use a
/// recording fake.
trait TrayTarget {
    fn set_icon(&mut self, kind: TrayIconKind) -> bool;
    fn set_tooltip(&mut self, tooltip: &str) -> bool;
    /// Sets the text, enabled state and (for a checkbox) checked state of the menu item `item.id`.
    fn set_item(&mut self, item: &MenuItemModel) -> bool;
}

/// Brings `tray` from `applied` to `next` by changing only what differs: a status update every second touches the
/// tooltip and the status line, not the icon or the other items. `applied` records what the tray shows. It is locked
/// from the comparison to the record, so it always matches the tray even if two updates overlap. Whatever fails to
/// change keeps its old value in `applied`, so the next update tries it again.
///
/// The lock is held across calls into the tray, which wait for the main thread. That is fine because only the main
/// thread gets here (see [`refresh`]), so nothing else waits on the lock.
fn sync(applied: &Mutex<TrayModel>, next: &TrayModel, tray: &mut impl TrayTarget) {
    let mut applied = applied.lock().unwrap_or_else(PoisonError::into_inner);
    if applied.icon != next.icon && tray.set_icon(next.icon) {
        applied.icon = next.icon;
    }
    if applied.tooltip != next.tooltip && tray.set_tooltip(&next.tooltip) {
        applied.tooltip.clone_from(&next.tooltip);
    }
    for entry in &next.entries {
        let MenuEntry::Item(item) = entry else { continue };
        if let Some(shown) = find_item(&mut applied.entries, item.id)
            && shown != item
            && tray.set_item(item)
        {
            shown.clone_from(item);
        }
    }
}

fn find_item<'a>(entries: &'a mut [MenuEntry], id: &str) -> Option<&'a mut MenuItemModel> {
    entries.iter_mut().find_map(|entry| match entry {
        MenuEntry::Item(item) if item.id == id => Some(item),
        _ => None,
    })
}

struct LiveTray<'a, R: Runtime> {
    tray: &'a TrayIcon<R>,
    view: &'a TrayView<R>,
}

impl<R: Runtime> TrayTarget for LiveTray<'_, R> {
    fn set_icon(&mut self, kind: TrayIconKind) -> bool {
        self.tray.set_icon(Some(self.view.icons.image(kind))).is_ok()
    }

    fn set_tooltip(&mut self, tooltip: &str) -> bool {
        self.tray.set_tooltip(Some(tooltip)).is_ok()
    }

    fn set_item(&mut self, item: &MenuItemModel) -> bool {
        match self.view.menu.get(item.id) {
            Some(MenuItemKind::MenuItem(menu_item)) => {
                let text = menu_item.set_text(&item.label);
                let enabled = menu_item.set_enabled(item.enabled);
                text.is_ok() && enabled.is_ok()
            }
            Some(MenuItemKind::Check(check)) => {
                let text = check.set_text(&item.label);
                let enabled = check.set_enabled(item.enabled);
                let checked = check.set_checked(item.checked.unwrap_or(false));
                text.is_ok() && enabled.is_ok() && checked.is_ok()
            }
            // The menu is built from the same model, so every item is there; nothing to retry otherwise.
            _ => true,
        }
    }
}

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
    let icons = TrayIcons::load()?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icons.image(model.icon))
        .tooltip(&model.tooltip)
        .menu(&menu)
        .show_menu_on_left_click(!LEFT_CLICK_OPENS_SETTINGS)
        .on_menu_event(|app, event| {
            if let Some(action) = AppAction::from_id(event.id().as_ref()) {
                actions::dispatch(action, app);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if LEFT_CLICK_OPENS_SETTINGS
                && let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
            {
                actions::dispatch(AppAction::OpenSettings, tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(TrayView {
        menu,
        icons,
        applied: Mutex::new(model),
    });
    Ok(())
}

/// Shows the current [`SharedStatus`] in the tray: icon, tooltip and menu items.
///
/// The update runs on the main thread, in the order the requests came in, and reads the status when it runs, not when
/// it was asked for. So two refreshes from different threads can't land out of order and leave the tray on a stale
/// state: the last one to run always shows the latest status. It also makes the icon bookkeeping single-threaded.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    // Best effort: without an event loop there is no tray to update.
    let _ = app.run_on_main_thread(move || apply_status(&handle));
}

/// The body of [`refresh`]; main thread only.
fn apply_status<R: Runtime>(app: &AppHandle<R>) {
    let next = current_model(app);
    // Before `setup` (and in tests without a tray) there is nothing to update.
    let (Some(view), Some(tray)) = (app.try_state::<TrayView<R>>(), app.tray_by_id(TRAY_ID)) else {
        return;
    };
    sync(
        &view.applied,
        &next,
        &mut LiveTray {
            tray: &tray,
            view: &view,
        },
    );
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
        assert_eq!(model.tooltip, "Repeat That - Capturing Discord · 24 s of speech");
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default());
        assert_eq!(model.tooltip, "Repeat That - No app selected");
    }

    fn bound_hotkeys() -> HotkeyLabels {
        HotkeyLabels {
            replay: Some("Ctrl+Alt+Space".into()),
            catch_up: Some("Ctrl+Alt+Enter".into()),
            stop: Some("Ctrl+Alt+Backspace".into()),
        }
    }

    #[test]
    fn no_hotkeys_are_shown_until_they_are_registered() {
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default());
        assert_eq!(item(&model, AppAction::Replay.id()).label, "Replay");
        assert_eq!(item(&model, AppAction::CatchUp.id()).label, "Catch-up");
        assert_eq!(item(&model, AppAction::Stop.id()).label, "Stop");
    }

    #[test]
    fn menu_layout() {
        let model = build_model(&capturing("Discord", 24), &bound_hotkeys());
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

    #[test]
    fn all_tray_icons_decode_to_distinct_images() {
        let icons = TrayIcons::load().expect("the embedded icons decode");
        let rgba = |kind| icons.image(kind).rgba().to_vec();
        assert_ne!(rgba(TrayIconKind::Idle), rgba(TrayIconKind::Capturing));
        assert_ne!(rgba(TrayIconKind::Idle), rgba(TrayIconKind::Paused));
        assert_ne!(rgba(TrayIconKind::Capturing), rgba(TrayIconKind::Paused));
    }

    /// A tray that records what it was asked to change. `fail` makes the call that matches fail.
    #[derive(Default)]
    struct FakeTray {
        calls: Vec<String>,
        fail: Option<&'static str>,
    }

    impl FakeTray {
        fn call(&mut self, call: String) -> bool {
            let works = self.fail != Some(call.as_str());
            self.calls.push(call);
            works
        }
    }

    impl TrayTarget for FakeTray {
        fn set_icon(&mut self, kind: TrayIconKind) -> bool {
            self.call(format!("icon {kind:?}"))
        }

        fn set_tooltip(&mut self, _tooltip: &str) -> bool {
            self.call("tooltip".into())
        }

        fn set_item(&mut self, item: &MenuItemModel) -> bool {
            self.call(format!("item {}", item.id))
        }
    }

    fn model(status: &TrayStatus) -> TrayModel {
        build_model(status, &HotkeyLabels::default())
    }

    /// Syncs a tray showing `from` to `to` and returns what was changed.
    fn changes(from: &TrayStatus, to: &TrayStatus) -> Vec<String> {
        let mut tray = FakeTray::default();
        sync(&Mutex::new(model(from)), &model(to), &mut tray);
        tray.calls
    }

    #[test]
    fn nothing_is_touched_when_nothing_changed() {
        assert!(changes(&capturing("Discord", 30), &capturing("Discord", 30)).is_empty());
    }

    #[test]
    fn a_tick_of_speech_only_touches_the_tooltip_and_the_status_line() {
        assert_eq!(
            changes(&capturing("Discord", 30), &capturing("Discord", 31)),
            ["tooltip", "item status"]
        );
    }

    #[test]
    fn pausing_touches_the_icon_the_status_line_and_the_checkbox() {
        let mut paused = capturing("Discord", 30);
        paused.set_paused(true);
        assert_eq!(
            changes(&capturing("Discord", 30), &paused),
            ["icon Paused", "tooltip", "item status", "item toggle_pause"]
        );
    }

    #[test]
    fn changed_hotkey_labels_touch_only_their_items() {
        let status = TrayStatus::default();
        let hotkeys = HotkeyLabels {
            replay: Some("Ctrl+R".into()),
            ..HotkeyLabels::default()
        };
        let mut tray = FakeTray::default();
        sync(&Mutex::new(model(&status)), &build_model(&status, &hotkeys), &mut tray);
        assert_eq!(tray.calls, ["item replay"]);
    }

    #[test]
    fn a_failed_change_is_tried_again_and_the_rest_is_not() {
        let mut paused = capturing("Discord", 30);
        paused.set_paused(true);
        let applied = Mutex::new(model(&capturing("Discord", 30)));
        let next = model(&paused);

        let mut broken = FakeTray {
            fail: Some("icon Paused"),
            ..FakeTray::default()
        };
        sync(&applied, &next, &mut broken);
        assert_eq!(
            broken.calls,
            ["icon Paused", "tooltip", "item status", "item toggle_pause"]
        );

        let mut working = FakeTray::default();
        sync(&applied, &next, &mut working);
        assert_eq!(working.calls, ["icon Paused"]);
        sync(&applied, &next, &mut working);
        assert_eq!(working.calls, ["icon Paused"]);
    }

    #[test]
    fn what_is_recorded_is_what_the_tray_got_last_even_when_updates_overlap() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // A tray that remembers the icon it got last, by index into `KINDS`.
        struct IconTray<'a>(&'a AtomicUsize);
        impl TrayTarget for IconTray<'_> {
            fn set_icon(&mut self, kind: TrayIconKind) -> bool {
                self.0
                    .store(KINDS.iter().position(|k| *k == kind).unwrap(), Ordering::SeqCst);
                true
            }
            fn set_tooltip(&mut self, _: &str) -> bool {
                true
            }
            fn set_item(&mut self, _: &MenuItemModel) -> bool {
                true
            }
        }
        const KINDS: [TrayIconKind; 3] = [TrayIconKind::Idle, TrayIconKind::Capturing, TrayIconKind::Paused];

        let applied = Mutex::new(model(&TrayStatus::default()));
        let on_tray = AtomicUsize::new(0);
        let models = KINDS.map(|kind| TrayModel {
            icon: kind,
            ..model(&TrayStatus::default())
        });

        std::thread::scope(|scope| {
            for thread in 0..4 {
                let (applied, on_tray, models) = (&applied, &on_tray, &models);
                scope.spawn(move || {
                    for step in 0..2000 {
                        sync(applied, &models[(thread + step) % KINDS.len()], &mut IconTray(on_tray));
                    }
                });
            }
        });

        assert_eq!(applied.lock().unwrap().icon, KINDS[on_tray.load(Ordering::SeqCst)]);
    }
}
