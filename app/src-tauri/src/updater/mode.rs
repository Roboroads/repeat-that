//! What this install can do about updates: install them itself, only announce them, leave them to whatever installed
//! it, or not look for them at all. It depends on how the app was built and how it was installed, so it is worked out
//! once at startup and holds for the whole run.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::session::Env;

/// What the app does about updates, `UpdateStatus.mode` on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    /// Downloads and installs an update itself, and restarts.
    Install,
    /// Finds out that an update exists and points to the download page. It can't replace itself: a `.deb` or `.rpm`,
    /// a macOS app run from the disk image, an AppImage in a folder that can't be written.
    Notify,
    /// Something outside the app handles updates, so it never looks. Today that is the Flatpak, which has no network
    /// and comes from the GitHub releases only; once it is on Flathub, Flathub updates it.
    External,
    /// Doesn't look: this build has no update key, or it is a development build.
    Off,
}

/// The operating system, as far as the rules differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    pub const CURRENT: Os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Mac
    } else {
        Os::Linux
    };
}

/// What the build decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildFacts {
    /// An update public key is compiled in.
    pub has_key: bool,
    /// A debug build, which never talks to the real update server.
    pub debug_build: bool,
    /// A debug build that was started with a mock update source, which stands in for the server.
    pub mocked: bool,
    pub os: Os,
}

/// What the install looks like. The real one asks the system; the tests answer for it.
pub trait InstallFacts {
    /// The path of the running program.
    fn current_exe(&self) -> Option<PathBuf>;
    fn exists(&self, path: &Path) -> bool;
    /// Whether the file `file` can be replaced by a new version: it is there and its folder can be written to.
    fn can_replace(&self, file: &Path) -> bool;
}

/// The mode and, when it isn't `install`, the sentence that says why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    pub mode: UpdateMode,
    pub reason: Option<String>,
}

impl Detection {
    fn install() -> Self {
        Detection {
            mode: UpdateMode::Install,
            reason: None,
        }
    }

    fn with(mode: UpdateMode, reason: impl Into<String>) -> Self {
        Detection {
            mode,
            reason: Some(reason.into()),
        }
    }
}

pub const NO_KEY_REASON: &str = "This build doesn't check for updates.";
// The Flatpak is only on the GitHub releases for now. When it is on Flathub, this becomes a sentence about Flathub.
pub const FLATPAK_REASON: &str = "Download new versions of the Flatpak from the releases page on GitHub.";
pub const PACKAGE_REASON: &str = "Install updates from the download page or your package manager.";
pub const MOVE_REASON: &str = "Move Repeat That to Applications to get updates.";

/// The file Flatpak puts at the root of every sandbox.
const FLATPAK_INFO: &str = "/.flatpak-info";

/// How this install handles updates.
pub fn detect(build: BuildFacts, env: &dyn Env, facts: &dyn InstallFacts) -> Detection {
    // A Flatpak comes first: it never looks for updates, so that is what the user should be told, key or no key.
    if build.os == Os::Linux && (env.var("FLATPAK_ID").is_some() || facts.exists(Path::new(FLATPAK_INFO))) {
        return Detection::with(UpdateMode::External, FLATPAK_REASON);
    }
    if build.mocked {
        return Detection::install();
    }
    if !build.has_key || build.debug_build {
        return Detection::with(UpdateMode::Off, NO_KEY_REASON);
    }
    match build.os {
        Os::Windows => Detection::install(),
        Os::Mac => match facts.current_exe() {
            Some(exe) if runs_from_a_disk_image(&exe) => Detection::with(UpdateMode::Notify, MOVE_REASON),
            _ => Detection::install(),
        },
        Os::Linux => match env.var("APPIMAGE") {
            Some(appimage) if facts.can_replace(Path::new(&appimage)) => Detection::install(),
            Some(appimage) => Detection::with(
                UpdateMode::Notify,
                format!("Can't replace the AppImage at {appimage}, so new versions come from the download page."),
            ),
            None => Detection::with(UpdateMode::Notify, PACKAGE_REASON),
        },
    }
}

/// Whether a macOS app runs from a mounted disk image or from the random place macOS copies a downloaded app to
/// ("App Translocation"), neither of which the app can replace itself in.
fn runs_from_a_disk_image(exe: &Path) -> bool {
    let exe = exe.to_string_lossy();
    exe.starts_with("/Volumes/") || exe.contains("/AppTranslocation/")
}

/// The system's answers: the real files and the real folder permissions.
pub struct SystemFacts;

impl InstallFacts for SystemFacts {
    fn current_exe(&self) -> Option<PathBuf> {
        std::env::current_exe().ok()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn can_replace(&self, file: &Path) -> bool {
        file.is_file() && file.parent().is_some_and(folder_is_writable)
    }
}

/// Whether a file can be created in `dir`, which is what replacing a file in it takes. Tries it, with a file that is
/// gone again at once: permission bits alone say nothing about a read-only mount or an ACL.
fn folder_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".repeat-that-write-test-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct FakeEnv(HashMap<&'static str, &'static str>);

    impl FakeEnv {
        fn with(vars: &[(&'static str, &'static str)]) -> Self {
            FakeEnv(vars.iter().copied().collect())
        }
    }

    impl Env for FakeEnv {
        fn var(&self, name: &str) -> Option<String> {
            self.0
                .get(name)
                .filter(|value| !value.is_empty())
                .map(|value| value.to_string())
        }
    }

    #[derive(Default)]
    struct FakeFacts {
        exe: Option<&'static str>,
        existing: Vec<&'static str>,
        replaceable: Vec<&'static str>,
    }

    impl InstallFacts for FakeFacts {
        fn current_exe(&self) -> Option<PathBuf> {
            self.exe.map(PathBuf::from)
        }

        fn exists(&self, path: &Path) -> bool {
            self.existing.iter().any(|p| Path::new(p) == path)
        }

        fn can_replace(&self, file: &Path) -> bool {
            self.replaceable.iter().any(|p| Path::new(p) == file)
        }
    }

    /// A release build with a key, on `os`.
    fn shipped(os: Os) -> BuildFacts {
        BuildFacts {
            has_key: true,
            debug_build: false,
            mocked: false,
            os,
        }
    }

    fn detect_with(build: BuildFacts, env: &[(&'static str, &'static str)], facts: FakeFacts) -> Detection {
        detect(build, &FakeEnv::with(env), &facts)
    }

    #[test]
    fn a_build_without_a_key_looks_for_nothing() {
        for os in [Os::Windows, Os::Mac, Os::Linux] {
            let build = BuildFacts {
                has_key: false,
                ..shipped(os)
            };
            let found = detect_with(build, &[], FakeFacts::default());
            assert_eq!(found, Detection::with(UpdateMode::Off, NO_KEY_REASON), "{os:?}");
        }
    }

    #[test]
    fn a_debug_build_looks_for_nothing_even_with_a_key() {
        let build = BuildFacts {
            debug_build: true,
            ..shipped(Os::Windows)
        };
        assert_eq!(
            detect_with(build, &[], FakeFacts::default()),
            Detection::with(UpdateMode::Off, NO_KEY_REASON)
        );
    }

    #[test]
    fn the_mock_installs_whatever_the_system_and_whether_or_not_there_is_a_key() {
        for os in [Os::Windows, Os::Mac, Os::Linux] {
            for has_key in [false, true] {
                let build = BuildFacts {
                    has_key,
                    debug_build: true,
                    mocked: true,
                    os,
                };
                // On Linux without an AppImage a real build could only announce; the mock still installs.
                assert_eq!(
                    detect_with(build, &[], FakeFacts::default()),
                    Detection::install(),
                    "{os:?} {has_key}"
                );
            }
        }
    }

    #[test]
    fn windows_installs() {
        assert_eq!(
            detect_with(shipped(Os::Windows), &[], FakeFacts::default()),
            Detection::install()
        );
    }

    #[test]
    fn a_flatpak_is_sent_to_the_releases_page_for_updates() {
        // The Flatpak only comes from the GitHub releases for now. Once it is on Flathub, the sentence changes with it.
        assert_eq!(
            FLATPAK_REASON,
            "Download new versions of the Flatpak from the releases page on GitHub."
        );
        assert!(!FLATPAK_REASON.contains("Flathub"));
    }

    #[test]
    fn a_flatpak_is_found_by_its_variable_or_its_file() {
        let expected = Detection::with(UpdateMode::External, FLATPAK_REASON);
        let by_variable = detect_with(
            shipped(Os::Linux),
            &[("FLATPAK_ID", "me.roboroads.RepeatThat")],
            FakeFacts::default(),
        );
        assert_eq!(by_variable, expected);
        let by_file = detect_with(
            shipped(Os::Linux),
            &[],
            FakeFacts {
                existing: vec!["/.flatpak-info"],
                ..FakeFacts::default()
            },
        );
        assert_eq!(by_file, expected);
    }

    #[test]
    fn a_flatpak_says_so_even_without_a_key() {
        // The Flatpak is built without the update key, and the sentence that fits is still the Flatpak's.
        let build = BuildFacts {
            has_key: false,
            ..shipped(Os::Linux)
        };
        let found = detect_with(build, &[("FLATPAK_ID", "x")], FakeFacts::default());
        assert_eq!(found, Detection::with(UpdateMode::External, FLATPAK_REASON));
    }

    #[test]
    fn a_flatpak_wins_over_a_stray_appimage_variable() {
        let found = detect_with(
            shipped(Os::Linux),
            &[("FLATPAK_ID", "x"), ("APPIMAGE", "/a.AppImage")],
            FakeFacts {
                replaceable: vec!["/a.AppImage"],
                ..FakeFacts::default()
            },
        );
        assert_eq!(found.mode, UpdateMode::External);
    }

    #[test]
    fn an_appimage_that_can_be_replaced_installs() {
        let found = detect_with(
            shipped(Os::Linux),
            &[("APPIMAGE", "/home/me/Apps/Repeat-That.AppImage")],
            FakeFacts {
                replaceable: vec!["/home/me/Apps/Repeat-That.AppImage"],
                ..FakeFacts::default()
            },
        );
        assert_eq!(found, Detection::install());
    }

    #[test]
    fn an_appimage_that_cannot_be_replaced_only_announces() {
        let found = detect_with(
            shipped(Os::Linux),
            &[("APPIMAGE", "/opt/Repeat-That.AppImage")],
            FakeFacts::default(),
        );
        assert_eq!(found.mode, UpdateMode::Notify);
        let reason = found.reason.expect("a reason");
        assert!(reason.contains("/opt/Repeat-That.AppImage"), "{reason}");
        assert!(reason.starts_with("Can't replace the AppImage at"), "{reason}");
    }

    #[test]
    fn linux_without_an_appimage_only_announces() {
        // A .deb or .rpm, or a build somebody made themselves.
        let found = detect_with(shipped(Os::Linux), &[], FakeFacts::default());
        assert_eq!(found, Detection::with(UpdateMode::Notify, PACKAGE_REASON));
    }

    #[test]
    fn a_mac_app_run_from_the_disk_image_only_announces() {
        let found = detect_with(
            shipped(Os::Mac),
            &[],
            FakeFacts {
                exe: Some("/Volumes/Repeat That/Repeat That.app/Contents/MacOS/repeat-that"),
                ..FakeFacts::default()
            },
        );
        assert_eq!(found, Detection::with(UpdateMode::Notify, MOVE_REASON));
    }

    #[test]
    fn a_translocated_mac_app_only_announces() {
        let found = detect_with(
            shipped(Os::Mac),
            &[],
            FakeFacts {
                exe: Some(
                    "/private/var/folders/ab/xyz/AppTranslocation/1234/d/Repeat That.app/Contents/MacOS/repeat-that",
                ),
                ..FakeFacts::default()
            },
        );
        assert_eq!(found, Detection::with(UpdateMode::Notify, MOVE_REASON));
    }

    #[test]
    fn a_mac_app_in_applications_installs() {
        for exe in [
            "/Applications/Repeat That.app/Contents/MacOS/repeat-that",
            "/Users/me/Applications/Repeat That.app/Contents/MacOS/repeat-that",
            // A folder that merely has the word in its name is not the volume list.
            "/Users/me/Volumes/Repeat That.app/Contents/MacOS/repeat-that",
        ] {
            let found = detect_with(
                shipped(Os::Mac),
                &[],
                FakeFacts {
                    exe: Some(exe),
                    ..FakeFacts::default()
                },
            );
            assert_eq!(found, Detection::install(), "{exe}");
        }
    }

    #[test]
    fn a_mac_that_cannot_say_where_it_runs_from_installs() {
        assert_eq!(
            detect_with(shipped(Os::Mac), &[], FakeFacts::default()),
            Detection::install()
        );
    }

    #[test]
    fn the_flatpak_and_appimage_rules_are_for_linux_only() {
        // Neither variable means anything on the other systems, whatever the environment holds.
        for os in [Os::Windows, Os::Mac] {
            let found = detect_with(
                shipped(os),
                &[("FLATPAK_ID", "x"), ("APPIMAGE", "/a.AppImage")],
                FakeFacts::default(),
            );
            assert_eq!(found, Detection::install(), "{os:?}");
        }
    }

    #[test]
    fn only_a_reason_comes_with_a_mode_that_cannot_install() {
        for (build, env) in [
            (
                BuildFacts {
                    has_key: false,
                    ..shipped(Os::Linux)
                },
                vec![],
            ),
            (shipped(Os::Linux), vec![("FLATPAK_ID", "x")]),
            (shipped(Os::Linux), vec![]),
            (shipped(Os::Windows), vec![]),
        ] {
            let found = detect_with(build, &env, FakeFacts::default());
            assert_eq!(found.reason.is_some(), found.mode != UpdateMode::Install, "{found:?}");
        }
    }

    // The real answers

    #[test]
    fn a_folder_you_can_write_to_is_writable() {
        let dir = std::env::temp_dir();
        assert!(folder_is_writable(&dir));
    }

    #[test]
    fn a_folder_that_is_not_there_is_not_writable() {
        assert!(!folder_is_writable(
            &std::env::temp_dir().join("repeat-that-no-such-folder-here")
        ));
    }

    #[test]
    fn probing_a_folder_leaves_nothing_behind() {
        let dir = std::env::temp_dir().join(format!("repeat-that-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(folder_is_writable(&dir));
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(left.is_empty(), "{left:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_is_not_writable_and_its_file_cannot_be_replaced() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("repeat-that-readonly-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Repeat-That.AppImage");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let (writable, replaceable) = (folder_is_writable(&dir), SystemFacts.can_replace(&file));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        // The superuser writes anywhere, so there is nothing to see when the tests run as one.
        if !writable {
            assert!(!replaceable);
        }
    }

    #[test]
    fn a_file_that_is_there_in_a_writable_folder_can_be_replaced() {
        let dir = std::env::temp_dir().join(format!("repeat-that-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Repeat-That.AppImage");
        assert!(!SystemFacts.can_replace(&file), "not there yet");
        std::fs::write(&file, b"x").unwrap();
        let replaceable = SystemFacts.can_replace(&file);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(replaceable);
    }
}
