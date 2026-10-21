//! App icons for the app list: the icon of each app's executable as a PNG `data:` URL, made once per executable for the
//! whole session (an app's icon doesn't change while the app runs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rt_capture::{AudioApp, IconImage};

use crate::ipc::AppEntry;
use crate::locking::lock;

/// The size icons are made at. The window shows them at up to 36 px, so this stays sharp on a screen at 175%.
pub const ICON_PX: u32 = 64;

/// Reads an executable's icon; [`rt_capture::app_icon`] in the app, a fake in tests.
pub type IconReader = Box<dyn Fn(&Path) -> Option<IconImage> + Send + Sync>;

/// The icons made so far, by executable. An executable without an icon is remembered too, so it isn't read again.
pub struct IconCache {
    read: IconReader,
    by_exe: Mutex<HashMap<PathBuf, Option<String>>>,
}

/// The icon cache as managed app state.
pub type SharedIcons = Arc<IconCache>;

impl Default for IconCache {
    fn default() -> Self {
        IconCache::with_reader(Box::new(|exe| rt_capture::app_icon(exe, ICON_PX)))
    }
}

impl IconCache {
    pub fn with_reader(read: IconReader) -> Self {
        IconCache {
            read,
            by_exe: Mutex::default(),
        }
    }

    /// The icon of `exe` as a `data:` URL, or `None` if it has none. Reads the file the first time, so not on the UI
    /// thread.
    pub fn data_url(&self, exe: &Path) -> Option<String> {
        if let Some(known) = lock(&self.by_exe).get(exe) {
            return known.clone();
        }
        // Not locked while reading: another list can use the icons that are there meanwhile.
        let url = (self.read)(exe).and_then(|icon| png_data_url(&icon));
        lock(&self.by_exe).insert(exe.to_path_buf(), url.clone());
        url
    }

    /// `app` as the app list shows it, with its icon.
    pub fn entry(&self, app: AudioApp) -> AppEntry {
        let icon = app.exe_path.as_deref().and_then(|exe| self.data_url(exe));
        AppEntry {
            pid: app.pid,
            name: app.name,
            exe_path: app.exe_path.map(|path| path.to_string_lossy().into_owned()),
            icon,
        }
    }
}

/// `icon` as a `data:image/png;base64,…` URL; `None` if its pixels don't add up to its size.
pub fn png_data_url(icon: &IconImage) -> Option<String> {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, icon.width, icon.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&icon.rgba).ok()?;
        writer.finish().ok()?;
    }
    Some(format!("data:image/png;base64,{}", base64(&png)))
}

/// Standard base64 with padding (RFC 4648 §4).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk
            .iter()
            .enumerate()
            .fold(0u32, |group, (i, &byte)| group | u32::from(byte) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(group >> (18 - 6 * i) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn red_square(size: u32) -> IconImage {
        IconImage {
            width: size,
            height: size,
            rgba: [255, 0, 0, 255].repeat((size * size) as usize),
        }
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in cases {
            assert_eq!(base64(plain.as_bytes()), encoded, "{plain:?}");
        }
        assert_eq!(base64(&[0xfb, 0xff, 0xbf]), "+/+/");
    }

    #[test]
    fn an_icon_becomes_a_png_data_url_that_decodes_to_the_same_pixels() {
        let icon = red_square(4);
        let url = png_data_url(&icon).expect("encodes");
        let encoded = url.strip_prefix("data:image/png;base64,").expect("a PNG data URL");
        let png = decode_base64(encoded);
        let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (4, 4));
        assert_eq!(&pixels[..info.buffer_size()], icon.rgba.as_slice());
    }

    #[test]
    fn pixels_that_do_not_fit_the_size_make_no_icon() {
        let mut icon = red_square(4);
        icon.rgba.truncate(10);
        assert_eq!(png_data_url(&icon), None);
    }

    fn counting_cache(icon: Option<IconImage>) -> (IconCache, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&reads);
        let cache = IconCache::with_reader(Box::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            icon.clone()
        }));
        (cache, reads)
    }

    fn app(exe: Option<&str>) -> AudioApp {
        AudioApp {
            pid: 42,
            name: "Discord".into(),
            exe_path: exe.map(PathBuf::from),
        }
    }

    #[test]
    fn each_executable_is_read_once() {
        let (cache, reads) = counting_cache(Some(red_square(2)));
        let first = cache.entry(app(Some("C:\\Discord.exe")));
        let second = cache.entry(app(Some("C:\\Discord.exe")));
        assert!(first.icon.as_deref().unwrap().starts_with("data:image/png;base64,"));
        assert_eq!(first, second);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        cache.entry(app(Some("C:\\Teams.exe")));
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn an_executable_without_an_icon_is_remembered_too() {
        let (cache, reads) = counting_cache(None);
        assert_eq!(cache.entry(app(Some("C:\\NoIcon.exe"))).icon, None);
        assert_eq!(cache.entry(app(Some("C:\\NoIcon.exe"))).icon, None);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_app_without_an_executable_gets_the_generic_icon() {
        let (cache, reads) = counting_cache(Some(red_square(2)));
        let entry = cache.entry(app(None));
        assert_eq!(
            entry,
            AppEntry {
                pid: 42,
                name: "Discord".into(),
                exe_path: None,
                icon: None,
            }
        );
        assert_eq!(reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_real_reader_has_no_icon_for_a_missing_file() {
        assert_eq!(IconCache::default().data_url(Path::new("does/not/exist.exe")), None);
    }

    fn decode_base64(text: &str) -> Vec<u8> {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let digits: Vec<u32> = text
            .bytes()
            .filter(|&b| b != b'=')
            .map(|b| ALPHABET.iter().position(|&a| a == b).expect("a base64 digit") as u32)
            .collect();
        let mut bytes = Vec::new();
        for group in digits.chunks(4) {
            let value = group.iter().enumerate().fold(0, |v, (i, d)| v | d << (18 - 6 * i));
            bytes.extend((0..group.len() - 1).map(|i| (value >> (16 - 8 * i)) as u8));
        }
        bytes
    }
}
