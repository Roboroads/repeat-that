//! Opens the desktop's own page for keyboard shortcuts, for when the shortcuts portal can't be asked to (portal version
//! 1, which Plasma has) and the window would otherwise only say where to look.
//!
//! Which page depends on the desktop (`XDG_CURRENT_DESKTOP`) and on what is installed. Everything that looks at the
//! system or starts something sits behind [`System`], so the choice is tested without a desktop.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::session::{Env, ProcessEnv};

/// Programs that open the shortcut page of a desktop, best first, each with its arguments.
const KDE: &[&[&str]] = &[
    &["systemsettings", "kcm_keys"],
    &["systemsettings5", "kcm_keys"],
    &["kcmshell6", "kcm_keys"],
    &["kcmshell5", "kcm_keys"],
];
const GNOME: &[&[&str]] = &[&["gnome-control-center", "keyboard"]];

/// What the choice needs to know about the system, and starting a program.
pub trait System {
    /// Whether `program` is an executable on the `PATH` of this process.
    fn on_path(&self, program: &str) -> bool;
    /// Whether the host (outside the Flatpak sandbox) has `program` and the app may run things there. Both must hold:
    /// a sandbox can only ask the host to start something when it was given the permission for it.
    fn host_has(&self, program: &str) -> bool;
    /// Starts `argv` and returns without waiting for it to end.
    fn spawn(&self, argv: &[String]) -> Result<(), String>;
}

/// The command that opens the shortcut page of this desktop, or `None` when there is no way to: another desktop, or none
/// of the programs is there. In a Flatpak the program is looked for on the host and started through `flatpak-spawn`,
/// which the app only has when it was allowed to; without that there is no command, and the window keeps its text.
pub fn command(env: &dyn Env, system: &dyn System) -> Option<Vec<String>> {
    let desktops: Vec<String> = env
        .var("XDG_CURRENT_DESKTOP")?
        .split(':')
        .map(|name| name.trim().to_ascii_lowercase())
        .collect();
    let programs = if desktops.iter().any(|name| name == "kde") {
        KDE
    } else if desktops.iter().any(|name| name == "gnome") {
        GNOME
    } else {
        return None;
    };
    let in_flatpak = env.var("FLATPAK_ID").is_some();
    programs
        .iter()
        .find(|argv| {
            if in_flatpak {
                system.host_has(argv[0])
            } else {
                system.on_path(argv[0])
            }
        })
        .map(|argv| {
            let argv = argv.iter().map(|part| part.to_string());
            if in_flatpak {
                ["flatpak-spawn", "--host"]
                    .iter()
                    .map(|part| part.to_string())
                    .chain(argv)
                    .collect()
            } else {
                argv.collect()
            }
        })
}

/// Opens the shortcut page. Returns whether something was started, which is not whether the page came up.
pub fn open(env: &dyn Env, system: &dyn System) -> bool {
    let Some(argv) = command(env, system) else {
        return false;
    };
    match system.spawn(&argv) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("could not open the shortcut settings with {argv:?}: {err}");
            false
        }
    }
}

/// Opens the page, and says whether anything was started. The app's is [`real_opener`]; it is managed state so that the
/// commands can be tested without starting programs.
pub type PageOpener = Arc<dyn Fn() -> bool + Send + Sync>;

/// The opener of the real desktop.
pub fn real_opener() -> PageOpener {
    Arc::new(|| open(&ProcessEnv, &RealSystem))
}

/// The real system.
pub struct RealSystem;

impl System for RealSystem {
    fn on_path(&self, program: &str) -> bool {
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path).any(|dir| is_executable(&dir.join(program)))
    }

    fn host_has(&self, program: &str) -> bool {
        // `flatpak-spawn --host` fails when the app wasn't given the permission, as well as when the program is missing.
        Command::new("flatpak-spawn")
            .args(["--host", "sh", "-c", "command -v \"$1\" >/dev/null", "sh", program])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn spawn(&self, argv: &[String]) -> Result<(), String> {
        let (program, args) = argv.split_first().ok_or("there is nothing to start")?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| err.to_string())?;
        // Nobody waits for the page to be closed, but the process has to be collected when it ends.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
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
            self.0.get(name).map(|value| value.to_string())
        }
    }

    /// A system that has some programs here, some on the host, and records what it was asked to start.
    #[derive(Default)]
    struct FakeSystem {
        here: Vec<&'static str>,
        host: Vec<&'static str>,
        fail_to_spawn: bool,
        started: RefCell<Vec<Vec<String>>>,
    }

    impl FakeSystem {
        fn with_here(programs: &[&'static str]) -> Self {
            FakeSystem {
                here: programs.to_vec(),
                ..FakeSystem::default()
            }
        }

        fn with_host(programs: &[&'static str]) -> Self {
            FakeSystem {
                host: programs.to_vec(),
                ..FakeSystem::default()
            }
        }
    }

    impl System for FakeSystem {
        fn on_path(&self, program: &str) -> bool {
            self.here.contains(&program)
        }

        fn host_has(&self, program: &str) -> bool {
            self.host.contains(&program)
        }

        fn spawn(&self, argv: &[String]) -> Result<(), String> {
            self.started.borrow_mut().push(argv.to_vec());
            if self.fail_to_spawn {
                Err("no such file".into())
            } else {
                Ok(())
            }
        }
    }

    fn argv(parts: &[&str]) -> Option<Vec<String>> {
        Some(parts.iter().map(|part| part.to_string()).collect())
    }

    fn kde() -> FakeEnv {
        FakeEnv::with(&[("XDG_CURRENT_DESKTOP", "KDE")])
    }

    // KDE

    #[test]
    fn plasma_opens_the_shortcuts_module_in_system_settings() {
        assert_eq!(
            command(&kde(), &FakeSystem::with_here(&["systemsettings"])),
            argv(&["systemsettings", "kcm_keys"])
        );
    }

    #[test]
    fn without_system_settings_plasma_falls_back_to_the_module_on_its_own() {
        assert_eq!(
            command(&kde(), &FakeSystem::with_here(&["kcmshell6"])),
            argv(&["kcmshell6", "kcm_keys"])
        );
        assert_eq!(
            command(&kde(), &FakeSystem::with_here(&["kcmshell5"])),
            argv(&["kcmshell5", "kcm_keys"])
        );
    }

    #[test]
    fn the_newest_program_wins() {
        assert_eq!(
            command(
                &kde(),
                &FakeSystem::with_here(&["kcmshell5", "kcmshell6", "systemsettings5", "systemsettings"])
            ),
            argv(&["systemsettings", "kcm_keys"])
        );
        assert_eq!(
            command(&kde(), &FakeSystem::with_here(&["kcmshell6", "systemsettings5"])),
            argv(&["systemsettings5", "kcm_keys"])
        );
    }

    #[test]
    fn plasma_without_any_of_them_has_no_command() {
        assert_eq!(command(&kde(), &FakeSystem::with_here(&["gnome-control-center"])), None);
        assert_eq!(command(&kde(), &FakeSystem::default()), None);
    }

    // GNOME

    #[test]
    fn gnome_opens_the_keyboard_panel() {
        let env = FakeEnv::with(&[("XDG_CURRENT_DESKTOP", "GNOME")]);
        assert_eq!(
            command(&env, &FakeSystem::with_here(&["gnome-control-center"])),
            argv(&["gnome-control-center", "keyboard"])
        );
    }

    #[test]
    fn ubuntu_says_gnome_after_its_own_name() {
        // `XDG_CURRENT_DESKTOP` is a list.
        let env = FakeEnv::with(&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")]);
        assert_eq!(
            command(&env, &FakeSystem::with_here(&["gnome-control-center"])),
            argv(&["gnome-control-center", "keyboard"])
        );
    }

    #[test]
    fn gnome_without_its_control_center_has_no_command() {
        let env = FakeEnv::with(&[("XDG_CURRENT_DESKTOP", "GNOME")]);
        assert_eq!(command(&env, &FakeSystem::with_here(&["systemsettings"])), None);
    }

    // Anything else

    #[test]
    fn the_desktop_name_is_matched_whatever_its_case() {
        let env = FakeEnv::with(&[("XDG_CURRENT_DESKTOP", "Kde")]);
        assert!(command(&env, &FakeSystem::with_here(&["systemsettings"])).is_some());
    }

    #[test]
    fn a_program_of_another_desktop_is_never_started() {
        // KDE's programs are installed on many GNOME machines and the other way round.
        let gnome = FakeEnv::with(&[("XDG_CURRENT_DESKTOP", "GNOME")]);
        assert_eq!(
            command(&gnome, &FakeSystem::with_here(&["systemsettings", "kcmshell6"])),
            None
        );
        assert_eq!(command(&kde(), &FakeSystem::with_here(&["gnome-control-center"])), None);
    }

    #[test]
    fn other_desktops_and_none_have_no_command() {
        let everything = FakeSystem::with_here(&["systemsettings", "gnome-control-center", "xfce4-keyboard-settings"]);
        for desktop in ["sway", "Hyprland", "X-Cinnamon", "XFCE", "", "COSMIC", "KDEish"] {
            let env = FakeEnv::with(&[("XDG_CURRENT_DESKTOP", desktop)]);
            assert_eq!(command(&env, &everything), None, "{desktop:?}");
        }
        assert_eq!(command(&FakeEnv::default(), &everything), None);
    }

    // Flatpak

    fn flatpak_on(desktop: &'static str) -> FakeEnv {
        FakeEnv::with(&[
            ("XDG_CURRENT_DESKTOP", desktop),
            ("FLATPAK_ID", "me.roboroads.RepeatThat"),
        ])
    }

    #[test]
    fn in_a_flatpak_the_host_is_asked_to_open_the_page() {
        assert_eq!(
            command(&flatpak_on("KDE"), &FakeSystem::with_host(&["systemsettings"])),
            argv(&["flatpak-spawn", "--host", "systemsettings", "kcm_keys"])
        );
        assert_eq!(
            command(&flatpak_on("GNOME"), &FakeSystem::with_host(&["gnome-control-center"])),
            argv(&["flatpak-spawn", "--host", "gnome-control-center", "keyboard"])
        );
    }

    #[test]
    fn in_a_flatpak_the_sandboxs_own_path_does_not_count() {
        // The page would be opened on the host, where the program may not be; a program inside the sandbox is no use.
        assert_eq!(
            command(&flatpak_on("KDE"), &FakeSystem::with_here(&["systemsettings"])),
            None
        );
    }

    #[test]
    fn a_flatpak_that_may_not_run_things_on_the_host_has_no_command() {
        // `host_has` is false when the permission is missing, so the window keeps its text.
        assert_eq!(command(&flatpak_on("KDE"), &FakeSystem::default()), None);
        assert_eq!(command(&flatpak_on("GNOME"), &FakeSystem::default()), None);
    }

    #[test]
    fn in_a_flatpak_the_host_picks_the_program_like_anywhere() {
        assert_eq!(
            command(&flatpak_on("KDE"), &FakeSystem::with_host(&["kcmshell6"])),
            argv(&["flatpak-spawn", "--host", "kcmshell6", "kcm_keys"])
        );
    }

    // Opening it

    #[test]
    fn opening_starts_the_command_and_says_so() {
        let system = FakeSystem::with_here(&["systemsettings"]);
        assert!(open(&kde(), &system));
        assert_eq!(
            *system.started.borrow(),
            [vec!["systemsettings".to_string(), "kcm_keys".to_string()]]
        );
    }

    #[test]
    fn opening_without_a_command_starts_nothing() {
        let system = FakeSystem::default();
        assert!(!open(&kde(), &system));
        assert!(system.started.borrow().is_empty());
    }

    #[test]
    fn a_program_that_will_not_start_is_not_opened() {
        let system = FakeSystem {
            here: vec!["systemsettings"],
            fail_to_spawn: true,
            ..FakeSystem::default()
        };
        assert!(!open(&kde(), &system));
    }

    // The real system

    #[cfg(unix)]
    #[test]
    fn the_real_system_finds_programs_on_the_path_and_starts_them() {
        let system = RealSystem;
        assert!(system.on_path("sh"));
        assert!(!system.on_path("repeat-that-no-such-program"));
        assert!(system.spawn(&["true".to_string()]).is_ok());
        assert!(system.spawn(&["repeat-that-no-such-program".to_string()]).is_err());
        assert!(system.spawn(&[]).is_err());
    }
}
