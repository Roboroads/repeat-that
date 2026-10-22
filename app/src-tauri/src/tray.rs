//! The tray icon and its menu. [`build_model`] is a pure function from the app status to what the tray shows, so it can
//! be tested without a display; the rest turns a [`TrayModel`] into the real Tauri tray.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::actions::{self, AppAction};
use crate::ipc::{CaptureStateName, HotkeyBackendKind, HotkeyBindings, HotkeyState, HotkeyStates, Playback, Status};
use crate::settings::HOTKEY_ACTIONS;
use crate::source::UNSUPPORTED;
use crate::updater::{UpdateMode, UpdateState, UpdateStatus};

/// The window title and the start of the tooltip.
const APP_NAME: &str = "Repeat That";
/// The status line while the OS doesn't let the app capture audio. How to allow it is in the settings window.
const NOT_ALLOWED: &str = "Not allowed to capture audio";

/// Which of the three tray icons to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayIconKind {
    /// No app picked, waiting for the picked app, or an error.
    Idle,
    Capturing,
    Paused,
}

/// What capture is doing, as far as the tray cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureView {
    NoTarget,
    /// The picked app (by name) isn't running.
    Waiting(String),
    Capturing(String),
    Paused,
    /// Capture failed (and is tried again): what to tell the user.
    Error(String),
    /// The OS doesn't let the app capture audio (and it is tried again). How to allow it is in the settings window.
    NotAllowed,
    /// Capturing isn't possible here: why.
    Unsupported(String),
}

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
    /// What the tray shows for `status`: what the engine says, as the window shows it.
    pub fn from_status(status: &Status) -> Self {
        let name = || status.target_name.clone().unwrap_or_else(|| "the app".into());
        let capture = match status.capture.state {
            CaptureStateName::NoTarget => CaptureView::NoTarget,
            CaptureStateName::Waiting => CaptureView::Waiting(name()),
            CaptureStateName::Capturing => CaptureView::Capturing(name()),
            CaptureStateName::Paused => CaptureView::Paused,
            // The tooltip is short; the settings window has the details.
            CaptureStateName::Error => CaptureView::Error(format!("Can't capture {}", name())),
            CaptureStateName::NotAllowed => CaptureView::NotAllowed,
            CaptureStateName::Unsupported => {
                CaptureView::Unsupported(status.capture.message.clone().unwrap_or_else(|| UNSUPPORTED.into()))
            }
        };
        TrayStatus {
            capture,
            playback: match status.playback {
                Playback::Idle => PlaybackView::Idle,
                Playback::Replaying => PlaybackView::Replaying,
                Playback::CatchingUp => PlaybackView::CatchingUp,
            },
            buffered: Duration::from_millis(status.buffered_ms),
            behind: status.behind_ms.map(Duration::from_millis),
        }
    }

    pub fn is_paused(&self) -> bool {
        self.capture == CaptureView::Paused
    }

    /// Whether there is an app to pause, as for the window's Pause button. Pausing nothing would only pause the next
    /// app picked, without anything showing it.
    pub fn can_pause(&self) -> bool {
        !matches!(self.capture, CaptureView::NoTarget | CaptureView::Unsupported(_))
    }

    pub fn icon(&self) -> TrayIconKind {
        match self.capture {
            CaptureView::Capturing(_) => TrayIconKind::Capturing,
            CaptureView::Paused => TrayIconKind::Paused,
            CaptureView::NoTarget
            | CaptureView::Waiting(_)
            | CaptureView::Error(_)
            | CaptureView::NotAllowed
            | CaptureView::Unsupported(_) => TrayIconKind::Idle,
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
            CaptureView::NotAllowed => NOT_ALLOWED.to_string(),
            CaptureView::Error(message) | CaptureView::Unsupported(message) => message.clone(),
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

impl HotkeyLabels {
    /// The combinations from `bindings` that work: one another app holds, an invalid one or none shows no label, so
    /// the menu never names a hotkey that does nothing.
    pub fn from_states(bindings: &HotkeyBindings, states: &HotkeyStates) -> Self {
        let mut labels = HotkeyLabels::default();
        for (action, state) in HOTKEY_ACTIONS.into_iter().zip(states.slots()) {
            let label = match state {
                HotkeyState::Ok => bindings.accel(action).map(String::from),
                HotkeyState::InUse | HotkeyState::Invalid | HotkeyState::Unbound => None,
            };
            match action {
                AppAction::Replay => labels.replay = label,
                AppAction::CatchUp => labels.catch_up = label,
                AppAction::Stop => labels.stop = label,
                AppAction::TogglePause | AppAction::OpenSettings | AppAction::CheckForUpdates | AppAction::Quit => {}
            }
        }
        labels
    }
}

impl HotkeyLabels {
    /// The labels for `status`: the stored combinations that work with the plugin, the keys the desktop assigned under
    /// the portal, and none when there are no hotkeys.
    pub fn for_status(bindings: &HotkeyBindings, status: &Status) -> Self {
        match status.desktop.hotkey_backend {
            HotkeyBackendKind::Plugin => Self::from_states(bindings, &status.hotkeys),
            HotkeyBackendKind::Portal => {
                let [replay, stop, catch_up] = status.desktop.hotkey_triggers.slots().map(Clone::clone);
                HotkeyLabels { replay, catch_up, stop }
            }
            HotkeyBackendKind::None => HotkeyLabels::default(),
        }
    }
}

/// The slice of the update status the tray shows: where the update is, and which version once one is found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateTrayView {
    pub state: UpdateState,
    pub version: Option<String>,
}

impl UpdateTrayView {
    /// What the tray shows for `status`; `None` where the app never looks for updates (the menu has no item then, and
    /// that holds for the whole run, so the set of items never changes).
    pub fn from_status(status: &UpdateStatus) -> Option<Self> {
        let looks = matches!(status.mode, UpdateMode::Install | UpdateMode::Notify);
        looks.then(|| UpdateTrayView {
            state: status.state,
            version: status.available.as_ref().map(|update| update.version.clone()),
        })
    }

    /// The text of the menu item and whether it can be clicked.
    fn item(&self) -> (String, bool) {
        match self.state {
            UpdateState::Idle | UpdateState::UpToDate | UpdateState::Error => ("Check for updates…".into(), true),
            UpdateState::Checking => ("Checking for updates…".into(), false),
            UpdateState::Available => match &self.version {
                Some(version) => (format!("Update to {version}…"), true),
                None => ("Update available…".into(), true),
            },
            UpdateState::Downloading | UpdateState::Installing => ("Downloading update…".into(), false),
        }
    }
}

/// Everything the tray shows: the status, the hotkeys and the updates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrayState {
    pub status: TrayStatus,
    pub hotkeys: HotkeyLabels,
    /// `None` = no "Check for updates" item.
    pub updates: Option<UpdateTrayView>,
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

/// Everything the tray shows for `status`: icon, tooltip and menu. `updates` adds the "Check for updates" item, which
/// comes after Settings.
pub fn build_model(status: &TrayStatus, hotkeys: &HotkeyLabels, updates: Option<&UpdateTrayView>) -> TrayModel {
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
    let mut entries = vec![
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
            enabled: status.can_pause(),
            checked: Some(status.is_paused()),
        }),
        action(AppAction::OpenSettings, "Settings…", None),
    ];
    // Where the app looks for updates, which holds for the whole run, so the items of the menu never change.
    if let Some(updates) = updates {
        let (label, enabled) = updates.item();
        entries.push(MenuEntry::Item(MenuItemModel {
            id: AppAction::CheckForUpdates.id(),
            label,
            enabled,
            checked: None,
        }));
    }
    entries.push(MenuEntry::Separator);
    entries.push(action(AppAction::Quit, "Quit", None));
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

/// The shared state the tray shows. Locked briefly; never held across a call into the UI toolkit.
pub type SharedStatus = Mutex<TrayState>;

/// Locks the state, ignoring poisoning: a panic elsewhere must not take the tray down with it.
pub fn lock_status(status: &SharedStatus) -> MutexGuard<'_, TrayState> {
    status.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Creates the tray icon and menu. Needs [`SharedStatus`] in the managed state.
pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let model = current_model(app);
    let menu = build_menu(app, &model)?;
    let icons = TrayIcons::load()?;
    let builder = TrayIconBuilder::with_id(TRAY_ID)
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
        });
    // The Linux tray library hands the desktop's tray the icon as a file; in a Flatpak that has to be in a folder the
    // host can read too.
    #[cfg(target_os = "linux")]
    let builder = match crate::sandbox::tray_icon_dir() {
        Some(dir) => builder.temp_dir_path(dir),
        None => builder,
    };
    builder.build(app)?;
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
    // Before `setup` (and in tests without a tray) there is nothing to update.
    let (Some(view), Some(tray)) = (app.try_state::<TrayView<R>>(), app.tray_by_id(TRAY_ID)) else {
        return;
    };
    let next = current_model(app);
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
    let shared = app.state::<SharedStatus>();
    let state = lock_status(&shared);
    build_model(&state.status, &state.hotkeys, state.updates.as_ref())
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
    use crate::ipc::{CaptureStatus, DesktopStatus, HotkeyTriggers};

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

    fn paused() -> TrayStatus {
        TrayStatus {
            capture: CaptureView::Paused,
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
        assert_eq!(icon(Unsupported("no backend".into())), TrayIconKind::Idle);
        assert_eq!(icon(NotAllowed), TrayIconKind::Idle);
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
        let model = build_model(&capturing("Discord", 24), &HotkeyLabels::default(), None);
        assert_eq!(model.tooltip, "Repeat That - Capturing Discord · 24 s of speech");
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default(), None);
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
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default(), None);
        assert_eq!(item(&model, AppAction::Replay.id()).label, "Replay");
        assert_eq!(item(&model, AppAction::CatchUp.id()).label, "Catch-up");
        assert_eq!(item(&model, AppAction::Stop.id()).label, "Stop");
    }

    #[test]
    fn menu_layout() {
        let model = build_model(&capturing("Discord", 24), &bound_hotkeys(), None);
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

    fn updates(state: UpdateState, version: Option<&str>) -> UpdateTrayView {
        UpdateTrayView {
            state,
            version: version.map(String::from),
        }
    }

    fn check_item(state: UpdateState, version: Option<&str>) -> (String, bool) {
        let view = updates(state, version);
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default(), Some(&view));
        let item = item(&model, "check_updates");
        (item.label.clone(), item.enabled)
    }

    #[test]
    fn there_is_no_update_item_where_the_app_never_looks() {
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default(), None);
        assert!(items(&model).iter().all(|i| i.id != "check_updates"));
    }

    #[test]
    fn the_update_item_sits_after_settings() {
        let view = updates(UpdateState::Idle, None);
        let model = build_model(&capturing("Discord", 24), &bound_hotkeys(), Some(&view));
        let ids: Vec<&str> = model
            .entries
            .iter()
            .map(|e| match e {
                MenuEntry::Item(i) => i.id,
                MenuEntry::Separator => "---",
            })
            .collect();
        assert_eq!(
            ids,
            [
                "status",
                "replay",
                "catch_up",
                "stop",
                "---",
                "toggle_pause",
                "open_settings",
                "check_updates",
                "---",
                "quit"
            ]
        );
    }

    #[test]
    fn the_update_item_offers_a_check_until_one_runs() {
        for state in [UpdateState::Idle, UpdateState::UpToDate, UpdateState::Error] {
            assert_eq!(
                check_item(state, None),
                ("Check for updates…".to_string(), true),
                "{state:?}"
            );
        }
        assert_eq!(
            check_item(UpdateState::Checking, None),
            ("Checking for updates…".to_string(), false)
        );
    }

    #[test]
    fn the_update_item_names_the_version_that_was_found() {
        assert_eq!(
            check_item(UpdateState::Available, Some("0.3.0")),
            ("Update to 0.3.0…".to_string(), true)
        );
        assert_eq!(
            check_item(UpdateState::Available, None),
            ("Update available…".to_string(), true)
        );
    }

    #[test]
    fn the_update_item_waits_while_an_update_comes_in() {
        for state in [UpdateState::Downloading, UpdateState::Installing] {
            assert_eq!(
                check_item(state, Some("0.3.0")),
                ("Downloading update…".to_string(), false),
                "{state:?}"
            );
        }
    }

    #[test]
    fn the_update_item_runs_an_action() {
        let view = updates(UpdateState::Idle, None);
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default(), Some(&view));
        for i in items(&model).into_iter().filter(|i| i.id != STATUS_ID) {
            assert!(AppAction::from_id(i.id).is_some(), "{}", i.id);
        }
        assert_eq!(AppAction::from_id("check_updates"), Some(AppAction::CheckForUpdates));
    }

    fn update_status(mode: UpdateMode, state: UpdateState, version: Option<&str>) -> UpdateStatus {
        UpdateStatus {
            mode,
            mode_reason: None,
            current_version: "0.1.0".into(),
            state,
            available: version.map(|version| crate::updater::AvailableUpdate {
                version: version.into(),
                date: None,
                notes: String::new(),
                release_url: format!("https://github.com/Roboroads/repeat-that/releases/tag/v{version}"),
            }),
            progress: None,
            error: None,
            error_kind: None,
            last_checked_at: None,
            manual: false,
        }
    }

    #[test]
    fn the_tray_follows_the_update_status_where_the_app_looks() {
        for mode in [UpdateMode::Install, UpdateMode::Notify] {
            let status = update_status(mode, UpdateState::Available, Some("0.3.0"));
            assert_eq!(
                UpdateTrayView::from_status(&status),
                Some(updates(UpdateState::Available, Some("0.3.0"))),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn the_tray_has_no_update_item_where_the_app_never_looks() {
        for mode in [UpdateMode::External, UpdateMode::Off] {
            let status = update_status(mode, UpdateState::Idle, None);
            assert_eq!(UpdateTrayView::from_status(&status), None, "{mode:?}");
        }
    }

    #[test]
    fn a_change_of_update_state_touches_only_the_update_item() {
        let status = TrayStatus::default();
        let shown = build_model(
            &status,
            &HotkeyLabels::default(),
            Some(&updates(UpdateState::Idle, None)),
        );
        let next = build_model(
            &status,
            &HotkeyLabels::default(),
            Some(&updates(UpdateState::Checking, None)),
        );
        let mut tray = FakeTray::default();
        sync(&Mutex::new(shown), &next, &mut tray);
        assert_eq!(tray.calls, ["item check_updates"]);
    }

    #[test]
    fn status_line_item_is_disabled_and_the_rest_are_enabled() {
        let model = build_model(&capturing("Discord", 1), &HotkeyLabels::default(), None);
        for i in items(&model) {
            assert_eq!(i.enabled, i.id != STATUS_ID, "{}", i.id);
        }
    }

    #[test]
    fn pause_is_disabled_while_there_is_no_app_to_capture() {
        // Like the window's Pause button: pausing nothing would only pause the next app picked, unseen.
        let pause_enabled = |capture| {
            let status = TrayStatus {
                capture,
                ..TrayStatus::default()
            };
            item(
                &build_model(&status, &HotkeyLabels::default(), None),
                AppAction::TogglePause.id(),
            )
            .enabled
        };
        assert!(!pause_enabled(CaptureView::NoTarget));
        assert!(!pause_enabled(CaptureView::Unsupported(UNSUPPORTED.into())));
        assert!(pause_enabled(CaptureView::Waiting("Discord".into())));
        assert!(pause_enabled(CaptureView::Capturing("Discord".into())));
        assert!(pause_enabled(CaptureView::Paused), "resuming stays possible");
        assert!(pause_enabled(CaptureView::Error("Can't capture Discord".into())));
        assert!(pause_enabled(CaptureView::NotAllowed));
    }

    #[test]
    fn pause_item_is_a_checkbox_that_follows_the_state() {
        let hotkeys = HotkeyLabels::default();
        let running = build_model(&capturing("Discord", 1), &hotkeys, None);
        assert_eq!(item(&running, AppAction::TogglePause.id()).checked, Some(false));
        let model = build_model(&paused(), &hotkeys, None);
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
        let model = build_model(&TrayStatus::default(), &hotkeys, None);
        assert_eq!(item(&model, AppAction::Replay.id()).label, "Replay");
        assert_eq!(item(&model, AppAction::CatchUp.id()).label, "Catch-up (Ctrl+Alt+C)");
        assert_eq!(item(&model, AppAction::Stop.id()).label, "Stop");
    }

    #[test]
    fn every_menu_item_except_the_status_line_maps_to_an_action() {
        let model = build_model(&TrayStatus::default(), &HotkeyLabels::default(), None);
        for i in items(&model).into_iter().filter(|i| i.id != STATUS_ID) {
            assert!(AppAction::from_id(i.id).is_some(), "{}", i.id);
        }
    }

    fn wire(state: CaptureStateName, message: Option<&str>, target: Option<&str>) -> Status {
        Status {
            capture: CaptureStatus {
                state,
                message: message.map(String::from),
            },
            target_name: target.map(String::from),
            ..Status::default()
        }
    }

    fn view(status: &Status) -> CaptureView {
        TrayStatus::from_status(status).capture
    }

    #[test]
    fn the_capture_state_comes_from_the_status() {
        use CaptureStateName as S;
        let discord = Some("Discord");
        assert_eq!(view(&wire(S::NoTarget, None, None)), CaptureView::NoTarget);
        assert_eq!(
            view(&wire(S::Waiting, None, discord)),
            CaptureView::Waiting("Discord".into())
        );
        assert_eq!(
            view(&wire(S::Capturing, None, discord)),
            CaptureView::Capturing("Discord".into())
        );
        assert_eq!(view(&wire(S::Paused, None, discord)), CaptureView::Paused);
    }

    #[test]
    fn a_failing_capture_names_the_app_and_leaves_the_details_to_the_window() {
        let status = wire(
            CaptureStateName::Error,
            Some("audio backend error: 0x88890004"),
            Some("Discord"),
        );
        assert_eq!(view(&status), CaptureView::Error("Can't capture Discord".into()));
    }

    #[test]
    fn capture_that_is_not_allowed_says_so_and_leaves_how_to_allow_it_to_the_window() {
        let status = wire(
            CaptureStateName::NotAllowed,
            Some("Turn on Repeat That in System Settings > Privacy & Security > Screen & System Audio Recording."),
            Some("Discord"),
        );
        let tray = TrayStatus::from_status(&status);
        assert_eq!(tray.capture, CaptureView::NotAllowed);
        assert_eq!(tray.status_line(), "Not allowed to capture audio");
        assert_eq!(tray.icon(), TrayIconKind::Idle);
        let model = build_model(&tray, &HotkeyLabels::default(), None);
        assert_eq!(model.tooltip, "Repeat That - Not allowed to capture audio");
        assert_eq!(item(&model, STATUS_ID).label, "Not allowed to capture audio");
        // Pausing works, replays still play what was buffered, and Settings opens the window with the steps.
        for id in [
            AppAction::TogglePause.id(),
            AppAction::Replay.id(),
            AppAction::OpenSettings.id(),
        ] {
            assert!(item(&model, id).enabled, "{id}");
        }
        assert_eq!(item(&model, AppAction::TogglePause.id()).checked, Some(false));
    }

    #[test]
    fn an_unsupported_system_says_why() {
        let status = wire(CaptureStateName::Unsupported, Some("No capture on this OS"), None);
        assert_eq!(view(&status), CaptureView::Unsupported("No capture on this OS".into()));
        assert_eq!(TrayStatus::from_status(&status).status_line(), "No capture on this OS");
        let status = wire(CaptureStateName::Unsupported, None, None);
        assert_eq!(view(&status), CaptureView::Unsupported(UNSUPPORTED.into()));
    }

    #[test]
    fn a_target_without_a_name_is_the_app() {
        assert_eq!(
            view(&wire(CaptureStateName::Waiting, None, None)),
            CaptureView::Waiting("the app".into())
        );
    }

    #[test]
    fn playback_buffer_and_distance_to_live_come_from_the_status() {
        let status = Status {
            playback: Playback::Replaying,
            behind_ms: Some(2600),
            buffered_ms: 24_400,
            ..wire(CaptureStateName::Capturing, None, Some("Discord"))
        };
        let tray = TrayStatus::from_status(&status);
        assert_eq!(
            tray,
            TrayStatus {
                capture: CaptureView::Capturing("Discord".into()),
                playback: PlaybackView::Replaying,
                buffered: Duration::from_millis(24_400),
                behind: Some(Duration::from_millis(2600)),
            }
        );
        let model = build_model(&tray, &HotkeyLabels::default(), None);
        assert_eq!(model.tooltip, "Repeat That - Replaying · 3 s behind live");
        let catching_up = Status {
            playback: Playback::CatchingUp,
            ..status
        };
        assert_eq!(
            TrayStatus::from_status(&catching_up).status_line(),
            "Catching up · 3 s behind live"
        );
    }

    #[test]
    fn hotkey_labels_show_only_the_combinations_that_work() {
        let bindings = HotkeyBindings {
            replay: Some("Ctrl+Alt+Space".into()),
            stop: Some("Ctrl+Shift+Escape".into()),
            catch_up: Some("Ctrl+Alt+Frobnicate".into()),
        };
        let states = HotkeyStates {
            replay: HotkeyState::Ok,
            stop: HotkeyState::InUse,
            catch_up: HotkeyState::Invalid,
        };
        assert_eq!(
            HotkeyLabels::from_states(&bindings, &states),
            HotkeyLabels {
                replay: Some("Ctrl+Alt+Space".into()),
                catch_up: None,
                stop: None,
            }
        );
    }

    #[test]
    fn hotkey_labels_for_the_defaults() {
        let ok = HotkeyStates {
            replay: HotkeyState::Ok,
            stop: HotkeyState::Ok,
            catch_up: HotkeyState::Ok,
        };
        assert_eq!(
            HotkeyLabels::from_states(&HotkeyBindings::default(), &ok),
            bound_hotkeys()
        );
        let unbound = HotkeyBindings {
            replay: None,
            stop: None,
            catch_up: None,
        };
        assert_eq!(
            HotkeyLabels::from_states(&unbound, &HotkeyStates::UNBOUND),
            HotkeyLabels::default()
        );
    }

    fn status_on(backend: HotkeyBackendKind, hotkeys: HotkeyStates, triggers: HotkeyTriggers) -> Status {
        Status {
            hotkeys,
            desktop: DesktopStatus {
                hotkey_backend: backend,
                hotkey_triggers: triggers,
                ..DesktopStatus::default()
            },
            ..Status::default()
        }
    }

    fn all_ok() -> HotkeyStates {
        HotkeyStates {
            replay: HotkeyState::Ok,
            stop: HotkeyState::Ok,
            catch_up: HotkeyState::Ok,
        }
    }

    #[test]
    fn with_the_plugin_the_labels_are_the_stored_combinations_that_work() {
        let status = status_on(HotkeyBackendKind::Plugin, all_ok(), HotkeyTriggers::default());
        assert_eq!(
            HotkeyLabels::for_status(&HotkeyBindings::default(), &status),
            bound_hotkeys()
        );
    }

    #[test]
    fn under_the_portal_the_labels_are_the_keys_the_desktop_assigned() {
        // The stored combinations are only what was asked for; the desktop's answer is what works.
        let status = status_on(
            HotkeyBackendKind::Portal,
            HotkeyStates {
                replay: HotkeyState::Ok,
                stop: HotkeyState::Unbound,
                catch_up: HotkeyState::Ok,
            },
            HotkeyTriggers {
                replay: Some("Meta+Shift+R".into()),
                stop: None,
                catch_up: Some("Ctrl+Alt+Enter".into()),
            },
        );
        assert_eq!(
            HotkeyLabels::for_status(&HotkeyBindings::default(), &status),
            HotkeyLabels {
                replay: Some("Meta+Shift+R".into()),
                catch_up: Some("Ctrl+Alt+Enter".into()),
                stop: None,
            }
        );
    }

    #[test]
    fn under_the_portal_an_action_without_a_key_shows_the_bare_label() {
        let status = status_on(
            HotkeyBackendKind::Portal,
            HotkeyStates::UNBOUND,
            HotkeyTriggers::default(),
        );
        let labels = HotkeyLabels::for_status(&HotkeyBindings::default(), &status);
        assert_eq!(labels, HotkeyLabels::default());
        let model = build_model(&TrayStatus::default(), &labels, None);
        assert_eq!(item(&model, "replay").label, "Replay");
    }

    #[test]
    fn with_no_hotkeys_the_menu_names_none() {
        let status = status_on(HotkeyBackendKind::None, all_ok(), HotkeyTriggers::default());
        assert_eq!(
            HotkeyLabels::for_status(&HotkeyBindings::default(), &status),
            HotkeyLabels::default()
        );
    }

    #[test]
    fn a_portal_key_reaches_the_menu_item() {
        let status = status_on(
            HotkeyBackendKind::Portal,
            all_ok(),
            HotkeyTriggers {
                replay: Some("Meta+Shift+R".into()),
                stop: Some("Meta+Escape".into()),
                catch_up: None,
            },
        );
        let model = build_model(
            &TrayStatus::default(),
            &HotkeyLabels::for_status(&HotkeyBindings::default(), &status),
            None,
        );
        assert_eq!(item(&model, "replay").label, "Replay (Meta+Shift+R)");
        assert_eq!(item(&model, "stop").label, "Stop (Meta+Escape)");
        assert_eq!(item(&model, "catch_up").label, "Catch-up");
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
        build_model(status, &HotkeyLabels::default(), None)
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
        assert_eq!(
            changes(&capturing("Discord", 30), &paused()),
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
        sync(
            &Mutex::new(model(&status)),
            &build_model(&status, &hotkeys, None),
            &mut tray,
        );
        assert_eq!(tray.calls, ["item replay"]);
    }

    #[test]
    fn a_failed_change_is_tried_again_and_the_rest_is_not() {
        let applied = Mutex::new(model(&capturing("Discord", 30)));
        let next = model(&paused());

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
