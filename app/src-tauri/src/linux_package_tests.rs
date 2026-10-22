//! What the Linux package installs for the desktop to show: the desktop entry, its icon, and the names they go by.
//! `tauri.linux.conf.json` has the list; the bundler adds a desktop file and icons of its own, named after the product
//! and the binary, which these checks keep from being the ones a desktop uses.
//!
//! The icon is named in reverse DNS, after the app ID, on purpose. KDE looks an icon up in the current theme before it
//! looks in hicolor, and falls back to the part before the last dash when it finds nothing: `repeat-that` became
//! `repeat`, which Breeze has, a funnel with a red X. `me.roboroads.repeat` is nothing any theme has.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde_json::Value;

const APP_ID: &str = "me.roboroads.repeat-that";

fn read_json(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{path}: {err}"))).unwrap()
}

fn linux_config() -> Value {
    read_json("tauri.linux.conf.json")
}

/// Where each file the `.deb` installs by hand goes, and where it comes from. The `.rpm` installs the same files.
fn installed() -> BTreeMap<String, String> {
    installed_by("deb")
}

/// The same for one package: `deb`, `rpm` or `appimage` (the AppImage carries them inside).
fn installed_by(package: &str) -> BTreeMap<String, String> {
    linux_config()["bundle"]["linux"][package]["files"]
        .as_object()
        .unwrap_or_else(|| panic!("the files of the {package} package"))
        .iter()
        .map(|(destination, source)| (destination.clone(), source.as_str().unwrap().to_string()))
        .collect()
}

/// The list in a package's settings, such as `depends`.
fn listed(package: &str, key: &str) -> Vec<String> {
    linux_config()["bundle"]["linux"][package][key]
        .as_array()
        .unwrap_or_else(|| panic!("{package}.{key}"))
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect()
}

/// The desktop file the bundler makes itself, named after the product. The AppImage links it at its root.
fn bundler_desktop_file() -> String {
    let product = read_json("tauri.conf.json")["productName"]
        .as_str()
        .expect("a product name")
        .to_string();
    format!("/usr/share/applications/{product}.desktop")
}

/// The keys of the `[Desktop Entry]` group of a desktop file.
fn entry(path: &str) -> HashMap<String, String> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{path}: {err}"));
    let mut in_entry = false;
    let mut keys = HashMap::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry
            && !line.starts_with('#')
            && let Some((key, value)) = line.split_once('=')
        {
            keys.insert(key.to_string(), value.to_string());
        }
    }
    keys
}

fn desktop_file() -> String {
    installed()
        .get(&format!("/usr/share/applications/{APP_ID}.desktop"))
        .unwrap_or_else(|| panic!("{APP_ID}.desktop is not installed: {:?}", installed().keys()))
        .clone()
}

#[test]
fn the_app_id_is_the_identifier_and_what_the_portal_is_told() {
    assert_eq!(read_json("tauri.conf.json")["identifier"], APP_ID);
    #[cfg(target_os = "linux")]
    assert_eq!(crate::portal::HOST_APP_ID, APP_ID);
}

#[test]
fn every_file_the_package_is_built_from_exists() {
    let config = linux_config();
    let icons = config["bundle"]["icon"].as_array().expect("the icon list");
    let template = config["bundle"]["linux"]["deb"]["desktopTemplate"]
        .as_str()
        .expect("a template");
    let sources: Vec<String> = icons
        .iter()
        .map(|icon| icon.as_str().unwrap().to_string())
        .chain(
            ["deb", "rpm", "appimage"]
                .into_iter()
                .flat_map(|package| installed_by(package).into_values()),
        )
        .chain([template.to_string()])
        .collect();
    for source in sources {
        assert!(Path::new(&source).is_file(), "{source} is missing");
    }
}

#[test]
fn linux_builds_the_deb_the_rpm_and_the_appimage() {
    // The release takes exactly these three (release_assets.py has the patterns, build-linux.yml checks the files).
    assert_eq!(
        linux_config()["bundle"]["targets"],
        serde_json::json!(["deb", "rpm", "appimage"])
    );
}

#[test]
fn the_rpm_installs_what_the_deb_installs() {
    let config = linux_config();
    let linux = &config["bundle"]["linux"];
    assert_eq!(installed_by("rpm"), installed());
    assert_eq!(linux["rpm"]["desktopTemplate"], linux["deb"]["desktopTemplate"]);
}

#[test]
fn the_packages_depend_on_pipewire_and_alsa() {
    // The binary links libpipewire-0.3 (capture, ducking, replay output) and libasound (cpal's ALSA host). The bundler
    // adds WebKitGTK, GTK and the AppIndicator library itself, so only these two are listed. Debian 13 and Ubuntu 24.04
    // renamed both packages for the 64-bit time_t transition, Debian 12 has the old names. The rpm names the sonames,
    // as the bundler does for its own, so any rpm distribution resolves them (Fedora: pipewire-libs, alsa-lib).
    let deb = listed("deb", "depends");
    assert!(
        deb.contains(&"libpipewire-0.3-0t64 | libpipewire-0.3-0".to_string()),
        "{deb:?}"
    );
    assert!(deb.contains(&"libasound2t64 | libasound2".to_string()), "{deb:?}");
    let rpm = listed("rpm", "depends");
    assert!(rpm.contains(&"libpipewire-0.3.so.0()(64bit)".to_string()), "{rpm:?}");
    assert!(rpm.contains(&"libasound.so.2()(64bit)".to_string()), "{rpm:?}");
    // The PipeWire daemon and its session manager run on the desktop, so they are recommended rather than required: a
    // system with PulseAudio still installs the package and shows "Capture needs PipeWire".
    for package in ["deb", "rpm"] {
        assert_eq!(listed(package, "recommends"), ["pipewire", "wireplumber"], "{package}");
    }
}

#[test]
fn the_appimage_has_only_the_real_desktop_entry() {
    // The AppImage takes the bundler's own desktop file (named after the product) for its root, which is what an
    // AppImage integration tool installs. The hidden template would make that entry invisible, so the AppImage
    // replaces the file with the real entry and has no other.
    let appimage = installed_by("appimage");
    let entries: Vec<&String> = appimage
        .keys()
        .filter(|destination| destination.starts_with("/usr/share/applications/"))
        .collect();
    assert_eq!(entries, [&bundler_desktop_file()]);
    assert_eq!(appimage[&bundler_desktop_file()], desktop_file());
}

#[test]
fn the_appimage_carries_the_icons_of_the_packages() {
    let icons = |files: BTreeMap<String, String>| -> BTreeMap<String, String> {
        files
            .into_iter()
            .filter(|(destination, _)| destination.starts_with("/usr/share/icons/"))
            .collect()
    };
    assert_eq!(icons(installed_by("appimage")), icons(installed()));
}

#[test]
fn the_desktop_file_is_installed_under_the_app_id() {
    // The ID of a desktop file is its name, and the desktop matches the app's windows and the portal's shortcuts to it.
    let source = desktop_file();
    assert!(Path::new(&source).is_file());
    assert!(source.ends_with(&format!("{APP_ID}.desktop")), "{source}");
}

#[test]
fn its_entry_names_the_icon_after_the_app_id_and_starts_the_binary() {
    let keys = entry(&desktop_file());
    assert_eq!(keys["Type"], "Application");
    assert_eq!(keys["Name"], "Repeat That");
    // The name the app gives its shortcuts in KDE.
    #[cfg(target_os = "linux")]
    assert_eq!(keys["Name"], crate::kglobalaccel::APP_NAME);
    assert_eq!(keys["Icon"], APP_ID);
    assert_eq!(keys["Exec"], "repeat-that");
    assert_eq!(keys["Terminal"], "false");
    assert_eq!(keys["Categories"], "AudioVideo;Audio;Utility;");
    assert!(
        !keys.contains_key("NoDisplay") && !keys.contains_key("Hidden"),
        "{keys:?}"
    );
}

#[test]
fn its_entry_matches_the_class_the_window_has_on_x11() {
    // The window's WM_CLASS is the binary's name (`repeat-that`), and a taskbar finds the entry through this.
    assert_eq!(entry(&desktop_file())["StartupWMClass"], "repeat-that");
}

#[test]
fn the_window_takes_the_app_id_as_its_wayland_app_id() {
    // What lets a Wayland desktop tie the window to the desktop file named after it.
    assert_eq!(linux_config()["app"]["enableGTKAppId"], true);
}

#[test]
fn every_icon_is_installed_under_the_app_id() {
    let destinations: Vec<String> = installed().into_keys().collect();
    for wanted in [
        format!("/usr/share/icons/hicolor/scalable/apps/{APP_ID}.svg"),
        format!("/usr/share/icons/hicolor/32x32/apps/{APP_ID}.png"),
        format!("/usr/share/icons/hicolor/128x128/apps/{APP_ID}.png"),
        format!("/usr/share/icons/hicolor/256x256/apps/{APP_ID}.png"),
        format!("/usr/share/icons/hicolor/512x512/apps/{APP_ID}.png"),
        // Searched by every desktop whatever the theme says.
        format!("/usr/share/pixmaps/{APP_ID}.png"),
    ] {
        assert!(
            destinations.contains(&wanted),
            "{wanted} is not installed: {destinations:?}"
        );
    }
    for destination in &destinations {
        if destination.contains("/icons/") || destination.contains("/pixmaps/") {
            let file = destination.rsplit('/').next().unwrap();
            assert!(
                file == format!("{APP_ID}.png") || file == format!("{APP_ID}.svg"),
                "{destination}"
            );
        }
    }
}

#[test]
fn each_size_directory_holds_an_icon_of_that_size() {
    let size_of = |path: &str| {
        let png = std::fs::read(path).unwrap_or_else(|err| panic!("{path}: {err}"));
        // The width and height sit in the IHDR chunk, right after the 8-byte signature and the chunk's length and name.
        let number = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
        (number(16), number(20))
    };
    for (destination, source) in installed() {
        if let Some(rest) = destination.strip_prefix("/usr/share/icons/hicolor/") {
            let directory = rest.split('/').next().unwrap();
            if let Some((width, height)) = directory.split_once('x') {
                let expected: u32 = width.parse().unwrap();
                assert_eq!(height.parse::<u32>().unwrap(), expected);
                assert_eq!(size_of(&source), (expected, expected), "{destination}");
            }
        }
    }
}

#[test]
fn the_name_cannot_fall_back_to_a_theme_icon() {
    // What KDE tries when it finds no icon of the full name is the name up to its last dash.
    let fallback = APP_ID.rsplit_once('-').unwrap().0;
    assert_eq!(fallback, "me.roboroads.repeat");
    assert!(fallback.contains('.'), "{fallback} looks like a plain theme icon name");
}

#[test]
fn the_desktop_file_the_bundler_makes_is_hidden_and_claims_nothing() {
    // The bundler names its own desktop file after the product (`Repeat That.desktop`) and cannot be told not to make
    // it, so it gets a template that hides it: it must not show twice in the menu, or win the window over the real one.
    let template = linux_config()["bundle"]["linux"]["deb"]["desktopTemplate"]
        .as_str()
        .expect("a template")
        .to_string();
    let keys = entry(&template);
    assert_eq!(keys["NoDisplay"], "true");
    assert_eq!(
        keys["Icon"], APP_ID,
        "or the funnel comes back for anything that does read it"
    );
    assert!(!keys.contains_key("StartupWMClass"), "{keys:?}");
    assert!(!keys.contains_key("Categories"), "{keys:?}");
}

#[test]
fn the_bundler_is_not_asked_for_an_icon_in_a_directory_that_does_not_match_its_size() {
    // `128x128@2x.png` is 256 px, which the bundler files under `256x256@2`: the directory for a 512 px image.
    let config = linux_config();
    let icons: Vec<&str> = config["bundle"]["icon"]
        .as_array()
        .unwrap()
        .iter()
        .map(|icon| icon.as_str().unwrap())
        .collect();
    assert!(!icons.iter().any(|icon| icon.contains("@2x")), "{icons:?}");
}
