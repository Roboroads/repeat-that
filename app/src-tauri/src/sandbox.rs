//! Where the app puts the tray icon inside a Flatpak sandbox (Linux only). That the app runs in one is
//! `rt_capture::in_flatpak`; see the Flatpak notes in `docs/ARCHITECTURE.md` for the rest of what differs there.

use std::path::{Path, PathBuf};

use crate::session::{Env, ProcessEnv};

/// Where the tray library should write the icon files, or `None` for its own choice. The desktop's tray (a separate
/// process outside the sandbox) opens the file by its path, and the library's default, `$XDG_RUNTIME_DIR/tray-icon`,
/// is inside the sandbox, so the tray would show a blank icon. Flatpak shares `$XDG_RUNTIME_DIR/app/$FLATPAK_ID`
/// between the sandbox and the host at the same path, which is where the icon goes.
pub fn tray_icon_dir() -> Option<PathBuf> {
    tray_icon_dir_in(&ProcessEnv, rt_capture::in_flatpak())
}

fn tray_icon_dir_in(env: &dyn Env, sandboxed: bool) -> Option<PathBuf> {
    if !sandboxed {
        return None;
    }
    let runtime = env.var("XDG_RUNTIME_DIR")?;
    let id = env.var("FLATPAK_ID")?;
    Some(Path::new(&runtime).join("app").join(id))
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
}
