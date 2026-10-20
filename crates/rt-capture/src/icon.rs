//! App icons: the icon of an executable as an RGBA image, for the app list.

use std::path::Path;

/// An icon as 8-bit RGBA with straight (not premultiplied) alpha, row by row from the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// The largest icon [`app_icon`] makes, 4 MiB of pixels.
const MAX_SIZE_PX: u32 = 1024;

/// The icon of the executable `exe` as a square image of `size_px` pixels, or `None` if there is none: the file is
/// missing or has no icon, `size_px` is 0 or over 1024, or this OS has no icon support yet (only Windows has).
///
/// Reads the file, so don't call it on a UI thread, and cache the result: the icon of an app doesn't change while it
/// runs.
pub fn app_icon(exe: &Path, size_px: u32) -> Option<IconImage> {
    if !(1..=MAX_SIZE_PX).contains(&size_px) {
        return None;
    }
    #[cfg(windows)]
    {
        crate::windows::icon::extract(exe, size_px)
    }
    #[cfg(not(windows))]
    {
        let _ = exe;
        None
    }
}

/// Whether any pixel of `bgra` (32-bit pixels) has alpha. Icons from before alpha existed have none: all of their
/// pixels have alpha 0 and the shape is in the AND mask.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn has_alpha(bgra: &[u8]) -> bool {
    bgra.as_chunks::<4>().0.iter().any(|p| p[3] != 0)
}

/// Gives the pixels of `bgra` the alpha of an icon's AND mask, which `mask` holds as 32-bit pixels too: black is part
/// of the icon, anything else is see-through.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn alpha_from_mask(bgra: &mut [u8], mask: &[u8]) {
    for (pixel, mask) in bgra.as_chunks_mut::<4>().0.iter_mut().zip(mask.as_chunks::<4>().0) {
        pixel[3] = if mask[..3].iter().all(|&c| c == 0) { 255 } else { 0 };
    }
}

/// An image from BGRA pixels, scaled to `size`×`size` if it isn't.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn from_bgra(width: u32, height: u32, mut bgra: Vec<u8>, size: u32) -> IconImage {
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    resized(
        IconImage {
            width,
            height,
            rgba: bgra,
        },
        size,
    )
}

/// `image` scaled to `size`×`size` by averaging the pixels each new pixel covers, weighted by their alpha so see-through
/// pixels don't tint their neighbours.
fn resized(image: IconImage, size: u32) -> IconImage {
    if image.width == size && image.height == size {
        return image;
    }
    let (src_w, src_h, size) = (image.width as usize, image.height as usize, size as usize);
    // The part of the source that pixel `i` of `size` covers.
    let span = |i: usize, src: usize| {
        (
            i as f32 * src as f32 / size as f32,
            (i + 1) as f32 * src as f32 / size as f32,
        )
    };
    let overlap = |(from, to): (f32, f32), i: usize| (to.min(i as f32 + 1.0) - from.max(i as f32)).max(0.0);

    let mut rgba = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        let rows = span(y, src_h);
        for x in 0..size {
            let columns = span(x, src_w);
            let (mut color, mut alpha, mut area) = ([0.0f32; 3], 0.0f32, 0.0f32);
            for sy in rows.0 as usize..(rows.1.ceil() as usize).min(src_h) {
                for sx in columns.0 as usize..(columns.1.ceil() as usize).min(src_w) {
                    let weight = overlap(rows, sy) * overlap(columns, sx);
                    let pixel = &image.rgba[(sy * src_w + sx) * 4..][..4];
                    let opacity = f32::from(pixel[3]) / 255.0 * weight;
                    for (sum, &channel) in color.iter_mut().zip(pixel) {
                        *sum += f32::from(channel) * opacity;
                    }
                    alpha += opacity;
                    area += weight;
                }
            }
            if alpha > 0.0 {
                rgba.extend(color.map(|sum| (sum / alpha).round() as u8));
                rgba.push((alpha / area * 255.0).round() as u8);
            } else {
                rgba.extend([0, 0, 0, 0]);
            }
        }
    }
    IconImage {
        width: size as u32,
        height: size as u32,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_that_does_not_exist_has_no_icon() {
        let missing = Path::new("this/does/not/exist.exe");
        assert_eq!(app_icon(missing, 32), None);
    }

    #[test]
    fn sizes_that_make_no_sense_have_no_icon() {
        let this_test = std::env::current_exe().unwrap();
        assert_eq!(app_icon(&this_test, 0), None);
        assert_eq!(app_icon(&this_test, MAX_SIZE_PX + 1), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn other_systems_have_no_icons_yet() {
        let this_test = std::env::current_exe().unwrap();
        assert_eq!(app_icon(&this_test, 32), None);
    }

    #[test]
    fn bgra_becomes_rgba() {
        let image = from_bgra(2, 2, [1, 2, 3, 4, 5, 6, 7, 8].repeat(2), 2);
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.rgba, [3, 2, 1, 4, 7, 6, 5, 8].repeat(2));
    }

    #[test]
    fn only_icons_without_any_alpha_take_it_from_the_mask() {
        assert!(!has_alpha(&[9, 9, 9, 0, 1, 1, 1, 0]));
        assert!(has_alpha(&[9, 9, 9, 0, 1, 1, 1, 128]));

        let mut pixels = vec![10, 20, 30, 0, 40, 50, 60, 0];
        // The first pixel is in the mask (black), the second is not (white).
        alpha_from_mask(&mut pixels, &[0, 0, 0, 0, 255, 255, 255, 0]);
        assert_eq!(pixels, [10, 20, 30, 255, 40, 50, 60, 0]);
    }

    #[test]
    fn an_image_of_the_right_size_is_left_alone() {
        let image = IconImage {
            width: 2,
            height: 2,
            rgba: (0..16).collect(),
        };
        assert_eq!(resized(image.clone(), 2), image);
    }

    #[test]
    fn scaling_down_averages_pixels() {
        let image = IconImage {
            width: 2,
            height: 2,
            rgba: [[0, 0, 0, 255], [200, 100, 0, 255]].concat().repeat(2),
        };
        let small = resized(image, 1);
        assert_eq!((small.width, small.height), (1, 1));
        assert_eq!(small.rgba, [100, 50, 0, 255]);
    }

    #[test]
    fn see_through_pixels_do_not_tint_the_average() {
        // Opaque red next to transparent green: red at half the alpha, not a muddy yellow.
        let image = IconImage {
            width: 2,
            height: 2,
            rgba: [[255, 0, 0, 255], [0, 255, 0, 0]].concat().repeat(2),
        };
        assert_eq!(resized(image, 1).rgba, [255, 0, 0, 128]);
    }

    #[test]
    fn scaling_up_repeats_pixels() {
        let image = IconImage {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        };
        let big = resized(image, 2);
        assert_eq!((big.width, big.height), (2, 2));
        assert_eq!(big.rgba, [1, 2, 3, 4].repeat(4));
    }

    #[test]
    fn scaling_a_fully_transparent_image_stays_transparent() {
        let image = IconImage {
            width: 4,
            height: 4,
            rgba: [7, 7, 7, 0].repeat(16),
        };
        assert_eq!(resized(image.clone(), 3).rgba, vec![0; 36]);
        assert_eq!(resized(image, 5).rgba, vec![0; 100]);
    }
}
