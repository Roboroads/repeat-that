//! The environment of programs of the host system that the app starts: a desktop's shortcut settings, the browser for
//! a link.
//!
//! An AppImage's launcher points the app at the libraries inside the AppImage. AppRun puts the AppImage's folders in
//! front of `LD_LIBRARY_PATH`, `PATH`, `XDG_DATA_DIRS` and a few more, and its GTK hook sets `GTK_PATH`,
//! `GIO_MODULE_DIR`, `GDK_PIXBUF_MODULE_FILE`, `GTK_THEME` and others. A host program that inherits that loads the
//! AppImage's GLib or D-Bus library instead of its own and fails: on Debian 13 `gio` stops at an undefined symbol and
//! `dbus-send` at a missing symbol version. So those programs get the environment with every entry that points into the
//! AppImage taken out again, which is what the host had before the AppImage started. The app keeps its own
//! environment: it and the WebKit processes it starts need those libraries.
//!
//! Outside an AppImage (no `APPIMAGE` and `APPDIR`) host programs get the app's environment unchanged.
//!
//! The hidden argument `--rt-host-env` prints that environment and exits, so CI can start real host programs with it.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::process::{Command, ExitCode, Stdio};

/// What the AppImage runtime sets for the app itself: none of it belongs to another program.
const RUNTIME_VARS: &[&str] = &["APPDIR", "APPIMAGE", "ARGV0", "OWD"];
/// The themes the GTK hook forces (`Adwaita` in the light or dark variant, after the desktop's colour scheme). It sets
/// `GTK_THEME` whatever it was, so a host program does better without it.
const HOOK_THEMES: &[&str] = &["Adwaita:light", "Adwaita:dark"];
/// The hidden argument that prints the environment host programs get.
const ARG: &str = "--rt-host-env";

/// The environment for a host program, from the app's `vars`, or `None` when it is the app's own (not an AppImage).
pub fn host_env<I>(vars: I) -> Option<Vec<(OsString, OsString)>>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let vars: Vec<(OsString, OsString)> = vars.into_iter().collect();
    let value = |name: &str| {
        vars.iter()
            .find(|(key, _)| key == name)
            .and_then(|(_, value)| value.to_str())
    };
    let appdir = value("APPDIR")?.trim_end_matches('/');
    // The mount point of the AppImage. Anything else (empty, relative, `/`) can't be told apart from the host's paths.
    if value("APPIMAGE").is_none_or(str::is_empty) || !appdir.starts_with('/') || appdir.len() < 2 {
        return None;
    }
    let inside = |entry: &str| entry == appdir || entry.strip_prefix(appdir).is_some_and(|rest| rest.starts_with('/'));
    let forced_theme = value("APPIMAGE_GTK_THEME").map(str::to_string);
    let appdir = appdir.to_string();
    let host = vars
        .iter()
        .filter_map(|(key, value)| {
            if RUNTIME_VARS.iter().any(|name| key == name) {
                return None;
            }
            let Some(text) = value.to_str() else {
                return Some((key.clone(), value.clone()));
            };
            if key == "GTK_THEME" && (HOOK_THEMES.contains(&text) || forced_theme.as_deref() == Some(text)) {
                return None;
            }
            if !text.contains(appdir.as_str()) {
                return Some((key.clone(), value.clone()));
            }
            // A list of paths (or a single one): keep the entries that are not the AppImage's. The empty entry AppRun
            // leaves when the variable was unset before goes too.
            let kept: Vec<&str> = text
                .split(':')
                .filter(|entry| !entry.is_empty() && !inside(entry))
                .collect();
            (!kept.is_empty()).then(|| (key.clone(), OsString::from(kept.join(":"))))
        })
        .collect();
    Some(host)
}

/// A [`Command`] for a program of the host, with the host's environment when the app runs from an AppImage.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    if let Some(env) = host_env(std::env::vars_os()) {
        // With PATH set on the command, the program is looked up on the host's PATH too.
        command.env_clear().envs(env);
    }
    command
}

/// Whether the app runs from an AppImage, so that host programs need [`command`]'s environment.
#[cfg(target_os = "linux")]
pub fn in_appimage() -> bool {
    host_env(std::env::vars_os()).is_some()
}

/// Opens `url` with the first of the desktop's openers that starts (`xdg-open`, `gio open`, ...), each with the host's
/// environment, and doesn't wait for it.
#[cfg(target_os = "linux")]
pub fn open_url(url: &str) -> Result<(), String> {
    let mut last_error = String::from("no program to open links with");
    for template in open::commands(url) {
        let mut opener = command(template.get_program());
        opener
            .args(template.get_args())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match opener.spawn() {
            Ok(mut child) => {
                // The browser outlives the call; the opener itself is collected when it ends.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            Err(err) => last_error = format!("{}: {err}", template.get_program().to_string_lossy()),
        }
    }
    Err(last_error)
}

/// Prints the environment host programs get, as `NAME=value` entries each ended by a NUL byte (like `env -0`), if this
/// process was started with [`ARG`], and returns the exit code: the caller should then exit with it. `None` otherwise.
pub fn run_if_requested() -> Option<ExitCode> {
    if !std::env::args_os().skip(1).any(|arg| arg == ARG) {
        return None;
    }
    let env = host_env(std::env::vars_os()).unwrap_or_else(|| std::env::vars_os().collect());
    let mut out = std::io::stdout().lock();
    for (key, value) in env {
        let mut entry = key;
        entry.push("=");
        entry.push(value);
        if out
            .write_all(entry.as_encoded_bytes())
            .and_then(|()| out.write_all(b"\0"))
            .is_err()
        {
            return Some(ExitCode::FAILURE);
        }
    }
    Some(if out.flush().is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPDIR: &str = "/tmp/.mount_RepeatAb12";

    fn vars(list: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        list.iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    fn get<'a>(env: &'a [(OsString, OsString)], name: &str) -> Option<&'a str> {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.to_str().unwrap())
    }

    /// What AppRun and the GTK hook make of a host environment: their folders in front, variables of their own.
    fn appimage_env() -> Vec<(OsString, OsString)> {
        let lib = format!(
            "{APPDIR}/usr/lib/:{APPDIR}/usr/lib/i386-linux-gnu/:{APPDIR}/usr/lib/x86_64-linux-gnu/:{APPDIR}/lib/:"
        );
        vars(&[
            ("APPIMAGE", "/home/me/Apps/Repeat-That_0.2.0_amd64.AppImage"),
            ("APPDIR", APPDIR),
            ("ARGV0", "./Repeat-That_0.2.0_amd64.AppImage"),
            ("OWD", "/home/me"),
            ("HOME", "/home/me"),
            (
                "PATH",
                &format!("{APPDIR}/usr/bin/:{APPDIR}/usr/sbin/:/usr/local/bin:/usr/bin"),
            ),
            ("LD_LIBRARY_PATH", &lib),
            (
                "XDG_DATA_DIRS",
                &format!("{APPDIR}/usr/share:/usr/share:/usr/local/share:/usr/share"),
            ),
            ("GTK_PATH", &format!("{APPDIR}//usr/lib/gtk-3.0")),
            ("GIO_MODULE_DIR", &format!("{APPDIR}//usr/lib/gio/modules")),
            (
                "GDK_PIXBUF_MODULE_FILE",
                &format!("{APPDIR}//usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"),
            ),
            ("GTK_DATA_PREFIX", APPDIR),
            ("PYTHONHOME", &format!("{APPDIR}/usr/")),
            ("GTK_THEME", "Adwaita:dark"),
            ("XDG_CURRENT_DESKTOP", "KDE"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
        ])
    }

    #[test]
    fn outside_an_appimage_the_environment_is_the_apps() {
        assert_eq!(host_env(vars(&[("PATH", "/usr/bin"), ("HOME", "/home/me")])), None);
        // A Flatpak or a plain package has neither variable; one of them alone isn't an AppImage either.
        assert_eq!(host_env(vars(&[("APPDIR", APPDIR)])), None);
        assert_eq!(host_env(vars(&[("APPIMAGE", "/x.AppImage")])), None);
        assert_eq!(host_env(vars(&[("APPIMAGE", ""), ("APPDIR", APPDIR)])), None);
    }

    #[test]
    fn an_appdir_that_could_be_anything_is_not_trusted() {
        for appdir in ["", "/", "relative/dir"] {
            assert_eq!(
                host_env(vars(&[("APPIMAGE", "/x.AppImage"), ("APPDIR", appdir)])),
                None,
                "{appdir:?}"
            );
        }
    }

    #[test]
    fn path_lists_lose_the_appimages_entries_and_keep_the_hosts_in_order() {
        let env = host_env(appimage_env()).unwrap();
        assert_eq!(get(&env, "PATH"), Some("/usr/local/bin:/usr/bin"));
        assert_eq!(
            get(&env, "XDG_DATA_DIRS"),
            Some("/usr/share:/usr/local/share:/usr/share")
        );
    }

    #[test]
    fn variables_that_only_point_into_the_appimage_go() {
        let env = host_env(appimage_env()).unwrap();
        for name in [
            "LD_LIBRARY_PATH",
            "GTK_PATH",
            "GIO_MODULE_DIR",
            "GDK_PIXBUF_MODULE_FILE",
            "GTK_DATA_PREFIX",
            "PYTHONHOME",
        ] {
            assert_eq!(get(&env, name), None, "{name}");
        }
    }

    #[test]
    fn an_ld_library_path_the_host_had_comes_back() {
        let mut env = appimage_env();
        env.retain(|(key, _)| key != "LD_LIBRARY_PATH");
        env.push((
            "LD_LIBRARY_PATH".into(),
            format!("{APPDIR}/usr/lib/:{APPDIR}/lib/:/opt/vendor/lib").into(),
        ));
        assert_eq!(get(&host_env(env).unwrap(), "LD_LIBRARY_PATH"), Some("/opt/vendor/lib"));
    }

    #[test]
    fn the_runtimes_own_variables_and_the_forced_theme_go() {
        let env = host_env(appimage_env()).unwrap();
        for name in ["APPIMAGE", "APPDIR", "ARGV0", "OWD", "GTK_THEME"] {
            assert_eq!(get(&env, name), None, "{name}");
        }
        let mut forced = appimage_env();
        forced.retain(|(key, _)| key != "GTK_THEME");
        forced.extend(vars(&[("APPIMAGE_GTK_THEME", "Breeze"), ("GTK_THEME", "Breeze")]));
        assert_eq!(get(&host_env(forced).unwrap(), "GTK_THEME"), None);
    }

    #[test]
    fn a_theme_the_hook_did_not_set_stays() {
        let mut env = appimage_env();
        env.retain(|(key, _)| key != "GTK_THEME");
        env.extend(vars(&[("GTK_THEME", "Breeze-Dark")]));
        assert_eq!(get(&host_env(env).unwrap(), "GTK_THEME"), Some("Breeze-Dark"));
    }

    #[test]
    fn everything_else_is_passed_on_unchanged() {
        let env = host_env(appimage_env()).unwrap();
        assert_eq!(get(&env, "HOME"), Some("/home/me"));
        assert_eq!(get(&env, "XDG_CURRENT_DESKTOP"), Some("KDE"));
        assert_eq!(
            get(&env, "DBUS_SESSION_BUS_ADDRESS"),
            Some("unix:path=/run/user/1000/bus")
        );
    }

    #[test]
    fn a_folder_that_only_starts_like_the_appdir_is_the_hosts() {
        // `/tmp/.mount_RepeatAb12x` is another folder, not one inside `/tmp/.mount_RepeatAb12`.
        let mut env = appimage_env();
        env.push(("CUSTOM_PATH".into(), format!("{APPDIR}x/bin:{APPDIR}/usr/bin").into()));
        assert_eq!(
            get(&host_env(env).unwrap(), "CUSTOM_PATH"),
            Some(&*format!("{APPDIR}x/bin"))
        );
    }

    #[test]
    fn a_trailing_slash_on_the_appdir_changes_nothing() {
        let mut env = appimage_env();
        env.retain(|(key, _)| key != "APPDIR");
        env.push(("APPDIR".into(), format!("{APPDIR}/").into()));
        assert_eq!(get(&host_env(env).unwrap(), "PATH"), Some("/usr/local/bin:/usr/bin"));
    }

    #[test]
    fn no_host_value_points_into_the_appimage() {
        for (key, value) in host_env(appimage_env()).unwrap() {
            assert!(!value.to_str().unwrap().contains(APPDIR), "{key:?}={value:?}");
        }
    }
}
