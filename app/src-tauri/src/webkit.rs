//! Settings the app gives WebKitGTK, which draws the settings window on Linux, through the environment variables it reads
//! when the first webview starts. They are set before the app builds its window, and never over what the user set.

/// What the app wants for each variable when the user hasn't chosen.
///
/// `WEBKIT_DISABLE_DMABUF_RENDERER=1`: WebKitGTK's DMA-BUF renderer draws nothing for some graphics stacks (it was seen
/// on a Debian 13 KDE session on X11). The window opens, with its title bar, and the page in it stays blank, dark, and
/// the tray and everything else go on working, so nothing says what is wrong. Without the DMA-BUF renderer WebKit
/// draws through the compositing path that works there. This is a small settings window, so what it costs in speed
/// does not matter.
const DEFAULTS: &[(&str, &str)] = &[("WEBKIT_DISABLE_DMABUF_RENDERER", "1")];

/// The variables to set and what to set them to: every default whose variable `is_set` says nobody has set. A variable
/// the user set to anything (`0` included) is theirs.
fn defaults_to_set(is_set: impl Fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    DEFAULTS.iter().copied().filter(|(name, _)| !is_set(name)).collect()
}

/// [`defaults_to_set`], handed to `set`.
fn apply_with(is_set: impl Fn(&str) -> bool, mut set: impl FnMut(&str, &str)) {
    for (name, value) in defaults_to_set(is_set) {
        set(name, value);
    }
}

/// Gives WebKitGTK the app's settings. Call it first thing in the app, before any thread or webview exists.
pub fn apply_defaults() {
    apply_with(
        |name| std::env::var_os(name).is_some(),
        |name, value| {
            // SAFETY: this runs at the start of `run`, when the process has only its main thread (the duck guard check
            // before it starts none), so nothing reads the environment at the same time.
            unsafe { std::env::set_var(name, value) };
        },
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn set_in(env: &HashMap<&str, &str>) -> impl Fn(&str) -> bool {
        let names: Vec<String> = env.keys().map(|name| name.to_string()).collect();
        move |name| names.iter().any(|set| set == name)
    }

    #[test]
    fn the_dmabuf_renderer_is_turned_off_when_nobody_chose() {
        assert_eq!(
            defaults_to_set(set_in(&HashMap::new())),
            [("WEBKIT_DISABLE_DMABUF_RENDERER", "1")]
        );
    }

    #[test]
    fn whatever_the_user_set_is_theirs() {
        // `0` turns the renderer back on, which is a choice too.
        for value in ["0", "1", ""] {
            let env = HashMap::from([("WEBKIT_DISABLE_DMABUF_RENDERER", value)]);
            assert!(defaults_to_set(set_in(&env)).is_empty(), "{value:?}");
        }
    }

    #[test]
    fn other_variables_of_the_user_change_nothing() {
        let env = HashMap::from([("WEBKIT_FORCE_VBLANK_TIMER", "1"), ("HOME", "/home/me")]);
        assert_eq!(defaults_to_set(set_in(&env)), [("WEBKIT_DISABLE_DMABUF_RENDERER", "1")]);
    }

    #[test]
    fn applying_sets_what_is_missing_and_only_that() {
        let mut set = Vec::new();
        apply_with(|_| false, |name, value| set.push((name.to_string(), value.to_string())));
        assert_eq!(set, [("WEBKIT_DISABLE_DMABUF_RENDERER".to_string(), "1".to_string())]);

        let mut set = Vec::new();
        apply_with(|_| true, |name, value| set.push((name.to_string(), value.to_string())));
        assert!(set.is_empty());
    }
}
