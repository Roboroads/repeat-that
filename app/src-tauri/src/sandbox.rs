//! The Flatpak sandbox (Linux only): whether the app runs in one, and the few things that work differently inside it.
//!
//! Inside a Flatpak the app sees its own `/tmp`, its own `$XDG_RUNTIME_DIR` and its own config folders, and reaches the
//! desktop only through the D-Bus names and sockets the manifest lists (`packaging/flatpak`). Three things follow:
//!
//! * the autostart entry the autostart plugin writes would land in the sandbox's config folder, where the desktop never
//!   looks, so the Background portal makes it (see [`crate::autostart::PortalAutostart`]);
//! * the tray icon is a file that the desktop's tray reads, so it goes in the one folder both sides see
//!   ([`tray_icon_dir`]);
//! * the duck guard, the process that restores a ducked app after a crash, would die with the sandbox, so the duck
//!   journal restores at the next start instead (`rt_capture` decides that itself).

use std::path::{Path, PathBuf};

use crate::session::{Env, ProcessEnv};

/// The file Flatpak puts at the root of every sandbox.
const FLATPAK_INFO: &str = "/.flatpak-info";

/// Whether the app runs inside a Flatpak sandbox: Flatpak sets `FLATPAK_ID` in the environment of the app, and
/// `/.flatpak-info` is in the sandbox whatever the environment (a program started with `env -i`, say).
pub fn in_flatpak() -> bool {
    in_flatpak_with(&ProcessEnv, Path::new(FLATPAK_INFO))
}

fn in_flatpak_with(env: &dyn Env, info: &Path) -> bool {
    env.var("FLATPAK_ID").is_some() || info.exists()
}

/// Where the tray library should write the icon files, or `None` for its own choice. The desktop's tray (a separate
/// process outside the sandbox) opens the file by its path, and the library's default, `$XDG_RUNTIME_DIR/tray-icon`,
/// is inside the sandbox, so the tray would show a blank icon. Flatpak shares `$XDG_RUNTIME_DIR/app/$FLATPAK_ID`
/// between the sandbox and the host at the same path, which is where the icon goes.
pub fn tray_icon_dir() -> Option<PathBuf> {
    tray_icon_dir_in(&ProcessEnv, in_flatpak())
}

fn tray_icon_dir_in(env: &dyn Env, sandboxed: bool) -> Option<PathBuf> {
    if !sandboxed {
        return None;
    }
    let runtime = env.var("XDG_RUNTIME_DIR")?;
    let id = env.var("FLATPAK_ID")?;
    Some(Path::new(&runtime).join("app").join(id))
}

/// Who makes the entry that starts the app with the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutostartKind {
    /// The autostart plugin: a file in `~/.config/autostart` (or the registry, or a launch agent).
    Plugin,
    /// The Background portal, which makes the entry on the host for a sandboxed app.
    Portal,
}

/// How this install starts with the system: through the portal in a Flatpak, the plugin everywhere else.
pub fn autostart_kind() -> AutostartKind {
    autostart_kind_for(in_flatpak())
}

fn autostart_kind_for(sandboxed: bool) -> AutostartKind {
    if sandboxed {
        AutostartKind::Portal
    } else {
        AutostartKind::Plugin
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

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

    /// A folder standing in for the root of the file system, with or without `.flatpak-info` in it.
    fn root(with_info: bool) -> (scratch::Dir, PathBuf) {
        let dir = scratch::Dir::new();
        let info = dir.path().join(".flatpak-info");
        if with_info {
            std::fs::write(&info, "[Application]\nname=me.roboroads.RepeatThat\n").unwrap();
        }
        (dir, info)
    }

    /// A scratch folder that is removed when it goes out of scope.
    mod scratch {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicUsize, Ordering};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Self {
                static COUNT: AtomicUsize = AtomicUsize::new(0);
                let path = std::env::temp_dir().join(format!(
                    "repeat-that-sandbox-{}-{}",
                    std::process::id(),
                    COUNT.fetch_add(1, Ordering::SeqCst)
                ));
                std::fs::create_dir_all(&path).unwrap();
                Dir(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn the_flatpak_id_in_the_environment_means_a_flatpak() {
        let (_dir, info) = root(false);
        let env = FakeEnv::with(&[("FLATPAK_ID", "me.roboroads.RepeatThat")]);
        assert!(in_flatpak_with(&env, &info));
    }

    #[test]
    fn the_flatpak_info_file_means_a_flatpak_whatever_the_environment_says() {
        let (_dir, info) = root(true);
        assert!(in_flatpak_with(&FakeEnv::with(&[]), &info));
    }

    #[test]
    fn neither_means_no_flatpak() {
        let (_dir, info) = root(false);
        assert!(!in_flatpak_with(&FakeEnv::with(&[("HOME", "/home/me")]), &info));
    }

    #[test]
    fn an_empty_flatpak_id_is_not_one() {
        let (_dir, info) = root(false);
        assert!(!in_flatpak_with(&FakeEnv::with(&[("FLATPAK_ID", "")]), &info));
    }

    #[test]
    fn the_tray_icon_goes_in_the_folder_the_sandbox_shares_with_the_host() {
        let env = FakeEnv::with(&[
            ("FLATPAK_ID", "me.roboroads.RepeatThat"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]);
        assert_eq!(
            tray_icon_dir_in(&env, true),
            Some(PathBuf::from("/run/user/1000/app/me.roboroads.RepeatThat"))
        );
    }

    #[test]
    fn outside_a_flatpak_the_tray_library_chooses_the_folder() {
        let env = FakeEnv::with(&[("XDG_RUNTIME_DIR", "/run/user/1000")]);
        assert_eq!(tray_icon_dir_in(&env, false), None);
    }

    #[test]
    fn without_a_runtime_folder_or_an_id_the_tray_library_chooses_too() {
        let id = FakeEnv::with(&[("FLATPAK_ID", "me.roboroads.RepeatThat")]);
        assert_eq!(tray_icon_dir_in(&id, true), None);
        let runtime = FakeEnv::with(&[("XDG_RUNTIME_DIR", "/run/user/1000")]);
        assert_eq!(tray_icon_dir_in(&runtime, true), None);
    }

    #[test]
    fn a_flatpak_starts_with_the_system_through_the_portal() {
        assert_eq!(autostart_kind_for(true), AutostartKind::Portal);
        assert_eq!(autostart_kind_for(false), AutostartKind::Plugin);
    }
}
