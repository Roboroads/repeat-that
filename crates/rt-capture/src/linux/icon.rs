//! App names and icons from `.desktop` files. An executable's file name is matched to the `.desktop` file that starts
//! it (its `Exec`, `TryExec`, `StartupWMClass` or file name), whose `Name=` names the app and whose `Icon=` is looked up
//! in the icon theme. PNG icons only: an app with only an SVG icon gets the generic one.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::file_name;
use crate::IconImage;
use crate::icon::resized;

/// How long a scan of the `.desktop` files is used before the next lookup scans again, for apps installed meanwhile.
const RESCAN_AFTER: Duration = Duration::from_secs(30);

/// The largest icon file read, against a broken or hostile file.
const MAX_ICON_BYTES: usize = 16 << 20;

/// What a `.desktop` file says about its app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DesktopEntry {
    /// The desktop file ID without `.desktop`, e.g. `firefox-esr` or `org.mozilla.firefox`.
    id: String,
    name: Option<String>,
    icon: Option<String>,
    /// Program names (file names) that start this app: from `Exec` and `TryExec`.
    programs: Vec<String>,
    wm_class: Option<String>,
    no_display: bool,
}

/// The `.desktop` files of the installed apps.
#[derive(Debug, Default)]
pub(super) struct DesktopIndex(Vec<DesktopEntry>);

impl DesktopIndex {
    /// Reads the `applications` folders under `data_dirs`. A file in an earlier folder hides one with the same ID in a
    /// later folder, as in the XDG spec.
    fn scan(data_dirs: &[PathBuf]) -> Self {
        let mut entries: HashMap<String, DesktopEntry> = HashMap::new();
        let mut order = Vec::new();
        for dir in data_dirs {
            let applications = dir.join("applications");
            let mut files = Vec::new();
            collect_desktop_files(&applications, &applications, 0, &mut files);
            for (id, path) in files {
                if entries.contains_key(&id) {
                    continue;
                }
                if let Some(entry) = fs::read_to_string(&path).ok().and_then(|text| parse(&id, &text)) {
                    order.push(id.clone());
                    entries.insert(id, entry);
                }
            }
        }
        Self(order.into_iter().filter_map(|id| entries.remove(&id)).collect())
    }

    /// The entry of the app whose executable or binary is called `program`.
    fn find(&self, program: &str) -> Option<&DesktopEntry> {
        if program.is_empty() {
            return None;
        }
        let by_program = |e: &&DesktopEntry| e.programs.iter().any(|p| p == program);
        let by_id = |e: &&DesktopEntry| {
            e.id.eq_ignore_ascii_case(program)
                || e.id
                    .rsplit_once('.')
                    .is_some_and(|(_, last)| last.eq_ignore_ascii_case(program))
        };
        let by_class = |e: &&DesktopEntry| e.wm_class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(program));
        // Shown entries first: a hidden helper entry of the same program has a less useful name.
        for shown in [true, false] {
            let candidates = || self.0.iter().filter(move |e| e.no_display != shown);
            if let Some(entry) = candidates()
                .find(by_program)
                .or_else(|| candidates().find(by_id))
                .or_else(|| candidates().find(by_class))
            {
                return Some(entry);
            }
        }
        None
    }
}

/// Collects `(desktop file ID, path)` for every `.desktop` file under `dir`; files in subfolders get the subfolder in
/// their ID, joined with `-`.
fn collect_desktop_files(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if depth < 4 {
                collect_desktop_files(root, &path, depth + 1, out);
            }
        } else if path.extension().is_some_and(|e| e == "desktop")
            && let Ok(relative) = path.with_extension("").strip_prefix(root)
        {
            let id = relative.to_string_lossy().replace('/', "-");
            out.push((id, path));
        }
    }
}

/// The `[Desktop Entry]` of a `.desktop` file, if it is an app that is shown or at least not hidden.
fn parse(id: &str, text: &str) -> Option<DesktopEntry> {
    let mut entry = DesktopEntry {
        id: id.to_owned(),
        ..Default::default()
    };
    let mut in_entry = false;
    let mut application = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Type" => application = value == "Application",
            "Name" => entry.name = Some(value.to_owned()).filter(|v| !v.is_empty()),
            "Icon" => entry.icon = Some(value.to_owned()).filter(|v| !v.is_empty()),
            "Exec" | "TryExec" => entry.programs.extend(program_of(value)),
            "StartupWMClass" => entry.wm_class = Some(value.to_owned()),
            "NoDisplay" => entry.no_display = value == "true",
            "Hidden" if value == "true" => return None,
            _ => {}
        }
    }
    application.then_some(entry)
}

/// The file name of the program an `Exec` line starts: past `env VAR=value` and, for a Flatpak, its `--command`.
fn program_of(exec: &str) -> Option<String> {
    let words = exec_words(exec);
    let mut words = words.as_slice();
    if words.first().is_some_and(|w| file_name(Path::new(w)) == "env") {
        words = &words[1..];
        while words.first().is_some_and(|w| w.contains('=') && !w.starts_with('-')) {
            words = &words[1..];
        }
    }
    let program = file_name(Path::new(words.first()?));
    if program == "flatpak" {
        let command = words.iter().find_map(|w| w.strip_prefix("--command="));
        return command.map(|c| file_name(Path::new(c)).to_owned());
    }
    Some(program.to_owned()).filter(|p| !p.is_empty())
}

/// The words of an `Exec` line: split at spaces, except inside double quotes, where a backslash escapes.
fn exec_words(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let (mut word, mut quoted, mut escaped, mut started) = (String::new(), false, false, false);
    for c in exec.chars() {
        if escaped {
            word.push(c);
            escaped = false;
        } else if c == '\\' && quoted {
            escaped = true;
        } else if c == '"' {
            (quoted, started) = (!quoted, true);
        } else if c.is_whitespace() && !quoted {
            if started || !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            started = false;
        } else {
            word.push(c);
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// Where `.desktop` files and icons are: `$XDG_DATA_HOME` (`~/.local/share`), then `$XDG_DATA_DIRS`
/// (`/usr/local/share:/usr/share`), which a Flatpak setup extends with its exports.
fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".local/share")));
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data_home
        .into_iter()
        .chain(dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from))
        .collect()
}

/// The latest scan of this system's `.desktop` files, and when it was made.
static INDEX: Mutex<Option<(Instant, Arc<DesktopIndex>)>> = Mutex::new(None);

/// The index of this system's `.desktop` files, scanned again when it is older than [`RESCAN_AFTER`], or always when
/// `rescan` is false and there is one: a lookup that can't wait for a scan, which reads every `.desktop` file, takes the
/// one there is.
fn index(rescan: bool) -> Arc<DesktopIndex> {
    let mut index = INDEX.lock().unwrap_or_else(|e| e.into_inner());
    match &*index {
        Some((scanned, index)) if !needs_scan(Some(scanned.elapsed()), rescan) => Arc::clone(index),
        _ => {
            let fresh = Arc::new(DesktopIndex::scan(&data_dirs()));
            *index = Some((Instant::now(), Arc::clone(&fresh)));
            fresh
        }
    }
}

/// Whether a lookup scans the `.desktop` files, given the age of the latest scan (`None` without one).
fn needs_scan(age: Option<Duration>, rescan: bool) -> bool {
    age.is_none_or(|age| rescan && age >= RESCAN_AFTER)
}

/// The `Name=` of the app whose executable or binary is called `program`.
pub(super) fn desktop_name(program: &str) -> Option<String> {
    index(true).find(program)?.name.clone()
}

/// [`desktop_name`] from the `.desktop` files as the latest lookup found them, without scanning them again: for the
/// steps of a fade, which run on the PipeWire thread, a graph cycle apart.
pub(super) fn known_desktop_name(program: &str) -> Option<String> {
    index(false).find(program)?.name.clone()
}

/// The icon of the app whose executable is `exe`, at `size`×`size` pixels.
pub(super) fn icon(exe: &Path, size: u32) -> Option<IconImage> {
    let entry_icon = index(true).find(file_name(exe))?.icon.clone()?;
    let dirs = data_dirs();
    let theme = current_theme();
    load(&find_icon(&entry_icon, &dirs, theme.as_deref(), size)?, size)
}

/// The icon theme the desktop uses, from GTK's or KDE's settings.
fn current_theme() -> Option<String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let setting = |file: &str, group: &str, key: &str| {
        let text = fs::read_to_string(config.join(file)).ok()?;
        ini_value(&text, group, key)
    };
    setting("gtk-3.0/settings.ini", "Settings", "gtk-icon-theme-name")
        .or_else(|| setting("gtk-4.0/settings.ini", "Settings", "gtk-icon-theme-name"))
        .or_else(|| setting("kdeglobals", "Icons", "Theme"))
}

fn ini_value(text: &str, group: &str, key: &str) -> Option<String> {
    let mut in_group = false;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_group = name == group;
        } else if in_group
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim().trim_matches('"').to_owned()).filter(|v| !v.is_empty());
        }
    }
    None
}

/// The PNG file for icon `name` (or the absolute path an `Icon=` gives): in `theme`, then in `hicolor`, under the
/// `icons` folder of each of `data_dirs`, the smallest at least `size` pixels, else the largest; then `pixmaps`.
fn find_icon(name: &str, data_dirs: &[PathBuf], theme: Option<&str>, size: u32) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return (path.extension().is_some_and(|e| e == "png") && path.is_file()).then(|| path.to_owned());
    }
    let themes = theme.into_iter().chain(Some("hicolor"));
    for theme in themes {
        let mut found: Vec<(u32, PathBuf)> = Vec::new();
        for dir in data_dirs {
            let theme_dir = dir.join("icons").join(theme);
            let Ok(sizes) = fs::read_dir(&theme_dir) else { continue };
            for size_dir in sizes.filter_map(|e| e.ok()) {
                let Some(px) = size_dir.file_name().to_str().and_then(size_of_dir) else {
                    continue;
                };
                let file = size_dir.path().join("apps").join(format!("{name}.png"));
                if file.is_file() {
                    found.push((px, file));
                }
            }
        }
        found.sort();
        let fitting = found.iter().find(|(px, _)| *px >= size).or(found.last());
        if let Some((_, file)) = fitting {
            return Some(file.clone());
        }
    }
    data_dirs
        .iter()
        .map(|dir| dir.join("pixmaps").join(format!("{name}.png")))
        .find(|file| file.is_file())
}

/// The pixel size of an icon theme folder: `48x48`, or `48x48@2` for twice that.
fn size_of_dir(name: &str) -> Option<u32> {
    let (size, scale) = match name.split_once('@') {
        Some((size, scale)) => (size, scale.parse::<u32>().ok()?),
        None => (name, 1),
    };
    let (w, h) = size.split_once('x')?;
    let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
    (w == h).then(|| w * scale)
}

/// Decodes the PNG at `path` into a `size`×`size` icon.
fn load(path: &Path, size: u32) -> Option<IconImage> {
    if fs::metadata(path).ok()?.len() > MAX_ICON_BYTES as u64 {
        return None;
    }
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path).ok()?));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    decoder.set_limits(png::Limits { bytes: MAX_ICON_BYTES });
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let pixels = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[l, a]| [l, l, l, a])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&l| [l, l, l, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    if rgba.len() != (info.width * info.height * 4) as usize || info.width == 0 || info.height == 0 {
        return None;
    }
    let image = IconImage {
        width: info.width,
        height: info.height,
        rgba,
    };
    Some(if image.width == image.height {
        resized(image, size)
    } else {
        resized(square(image), size)
    })
}

/// `image` centered on a transparent square.
fn square(image: IconImage) -> IconImage {
    let side = image.width.max(image.height);
    let (dx, dy) = ((side - image.width) / 2, (side - image.height) / 2);
    let mut rgba = vec![0; (side * side * 4) as usize];
    for y in 0..image.height {
        let from = (y * image.width * 4) as usize;
        let to = (((y + dy) * side + dx) * 4) as usize;
        let len = (image.width * 4) as usize;
        rgba[to..to + len].copy_from_slice(&image.rgba[from..from + len]);
    }
    IconImage {
        width: side,
        height: side,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary data folder (like `/usr/share`) for one test, removed afterwards.
    struct DataDir(PathBuf);

    impl DataDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rt-icon-test-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, relative: &str, contents: &[u8]) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
    }

    impl Drop for DataDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A `size`×`size` PNG of one color, made in memory.
    fn png(size: u32, color: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&color.repeat((size * size) as usize)).unwrap();
        writer.finish().unwrap();
        out
    }

    const APP: &str = "[Desktop Entry]\nType=Application\nName=Test App\nExec=/opt/test/bin/rt-test-app %U\n\
        Icon=rt-test-app\n[Desktop Action new]\nName=Not this one\nExec=other\n";

    #[test]
    fn only_a_lookup_that_may_wait_scans_again() {
        let old = Some(RESCAN_AFTER + Duration::from_secs(1));
        let recent = Some(Duration::from_secs(1));
        assert!(
            needs_scan(None, true) && needs_scan(None, false),
            "the first lookup scans"
        );
        assert!(needs_scan(old, true));
        assert!(!needs_scan(old, false), "a fade step takes the index there is");
        assert!(!needs_scan(recent, true) && !needs_scan(recent, false));
    }

    #[test]
    fn programs_come_from_exec_lines() {
        assert_eq!(program_of("/usr/bin/firefox-esr %u").as_deref(), Some("firefox-esr"));
        assert_eq!(
            program_of("env GDK_BACKEND=x11 discord --flag").as_deref(),
            Some("discord")
        );
        assert_eq!(
            program_of(
                "/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=firefox org.mozilla.firefox @@u %u @@"
            )
            .as_deref(),
            Some("firefox")
        );
        assert_eq!(program_of("\"/opt/My App/my-app\" --new").as_deref(), Some("my-app"));
        assert_eq!(exec_words(r#"a "b c" "d\"e" "" f"#), ["a", "b c", "d\"e", "", "f"]);
        assert_eq!(program_of(""), None);
    }

    #[test]
    fn apps_are_found_by_program_id_or_window_class() {
        let dir = DataDir::new("find");
        dir.write("applications/rt-test-app.desktop", APP.as_bytes());
        dir.write(
            "applications/org.example.Viewer.desktop",
            b"[Desktop Entry]\nType=Application\nName=Viewer\nExec=launcher --x\nStartupWMClass=RtViewer\n",
        );
        dir.write(
            "applications/vendor/helper.desktop",
            b"[Desktop Entry]\nType=Application\nName=Helper\nExec=rt-helper\nNoDisplay=true\n",
        );
        dir.write(
            "applications/gone.desktop",
            b"[Desktop Entry]\nType=Application\nName=Gone\nExec=rt-gone\nHidden=true\n",
        );
        dir.write(
            "applications/link.desktop",
            b"[Desktop Entry]\nType=Link\nName=Link\nURL=x\n",
        );
        let index = DesktopIndex::scan(std::slice::from_ref(&dir.0));
        let name = |program| index.find(program).and_then(|e| e.name.clone());
        assert_eq!(name("rt-test-app").as_deref(), Some("Test App"));
        assert_eq!(name("viewer").as_deref(), Some("Viewer"));
        assert_eq!(name("rtviewer").as_deref(), Some("Viewer"));
        assert_eq!(name("rt-helper").as_deref(), Some("Helper"));
        assert_eq!(index.find("rt-helper").map(|e| e.id.as_str()), Some("vendor-helper"));
        assert_eq!(name("rt-gone"), None);
        assert_eq!(name("link"), None);
        assert_eq!(name(""), None);
    }

    #[test]
    fn an_earlier_data_folder_hides_a_later_one() {
        let (user, system) = (DataDir::new("user"), DataDir::new("system"));
        user.write(
            "applications/rt-test-app.desktop",
            b"[Desktop Entry]\nType=Application\nName=Mine\nExec=rt-test-app\n",
        );
        system.write("applications/rt-test-app.desktop", APP.as_bytes());
        let index = DesktopIndex::scan(&[user.0.clone(), system.0.clone()]);
        assert_eq!(index.find("rt-test-app").and_then(|e| e.name.as_deref()), Some("Mine"));
    }

    #[test]
    fn the_icon_is_the_smallest_png_at_least_the_size_asked_for() {
        let dir = DataDir::new("theme");
        let dirs = std::slice::from_ref(&dir.0);
        dir.write("icons/hicolor/32x32/apps/rt-test-app.png", &png(32, [255, 0, 0, 255]));
        dir.write("icons/hicolor/64x64/apps/rt-test-app.png", &png(64, [0, 255, 0, 255]));
        dir.write(
            "icons/hicolor/256x256/apps/rt-test-app.png",
            &png(256, [0, 0, 255, 255]),
        );
        dir.write("icons/hicolor/scalable/apps/rt-test-app.svg", b"<svg/>");
        let found = |size| find_icon("rt-test-app", dirs, None, size);
        assert!(found(48).unwrap().ends_with("64x64/apps/rt-test-app.png"));
        assert!(found(16).unwrap().ends_with("32x32/apps/rt-test-app.png"));
        // Nothing large enough: the largest there is.
        assert!(found(512).unwrap().ends_with("256x256/apps/rt-test-app.png"));

        // The desktop's own theme comes first.
        dir.write("icons/Papirus/48x48@2/apps/rt-test-app.png", &png(96, [9, 9, 9, 255]));
        assert!(
            find_icon("rt-test-app", dirs, Some("Papirus"), 48)
                .unwrap()
                .ends_with("Papirus/48x48@2/apps/rt-test-app.png")
        );

        let icon = load(&found(48).unwrap(), 48).unwrap();
        assert_eq!((icon.width, icon.height), (48, 48));
        assert_eq!(&icon.rgba[..4], [0, 255, 0, 255]);
        assert_eq!(icon.rgba.len(), 48 * 48 * 4);
    }

    #[test]
    fn icons_can_be_pixmaps_or_absolute_paths_but_not_svg() {
        let dir = DataDir::new("pixmaps");
        let dirs = std::slice::from_ref(&dir.0);
        dir.write("pixmaps/rt-old-app.png", &png(48, [1, 2, 3, 255]));
        dir.write("pixmaps/rt-vector.svg", b"<svg/>");
        assert!(
            find_icon("rt-old-app", dirs, None, 32)
                .unwrap()
                .ends_with("pixmaps/rt-old-app.png")
        );
        let absolute = dir.0.join("pixmaps/rt-old-app.png");
        assert_eq!(find_icon(absolute.to_str().unwrap(), dirs, None, 32), Some(absolute));
        assert_eq!(find_icon("rt-vector", dirs, None, 32), None);
        let svg = dir.0.join("pixmaps/rt-vector.svg");
        assert_eq!(find_icon(svg.to_str().unwrap(), dirs, None, 32), None);
    }

    #[test]
    fn a_wide_icon_is_centered_on_a_square() {
        let image = IconImage {
            width: 2,
            height: 1,
            rgba: [[1, 1, 1, 255], [2, 2, 2, 255]].concat(),
        };
        let square = square(image);
        assert_eq!((square.width, square.height), (2, 2));
        assert_eq!(square.rgba, [[1, 1, 1, 255], [2, 2, 2, 255], [0; 4], [0; 4]].concat());
    }

    #[test]
    fn theme_settings_are_read_from_their_group() {
        let gtk = "[Settings]\ngtk-theme-name=Adwaita\ngtk-icon-theme-name = Papirus-Dark\n";
        assert_eq!(
            ini_value(gtk, "Settings", "gtk-icon-theme-name").as_deref(),
            Some("Papirus-Dark")
        );
        let kde = "[General]\nTheme=wrong\n[Icons]\nTheme=breeze\n";
        assert_eq!(ini_value(kde, "Icons", "Theme").as_deref(), Some("breeze"));
        assert_eq!(ini_value(kde, "Icons", "Other"), None);
    }

    #[test]
    fn folder_names_give_icon_sizes() {
        assert_eq!(size_of_dir("48x48"), Some(48));
        assert_eq!(size_of_dir("24x24@2"), Some(48));
        assert_eq!(size_of_dir("scalable"), None);
        assert_eq!(size_of_dir("16x24"), None);
    }
}
