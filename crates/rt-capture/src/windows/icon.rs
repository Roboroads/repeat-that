//! App icons: the shell extracts an executable's icon at the size we ask for, as a GDI icon that we read out as pixels.
//!
//! GDI keeps an icon as a color bitmap and an AND mask. Modern icons have alpha in the color bitmap, which GDI keeps
//! straight (not premultiplied). Older ones don't, and their shape is in the mask.

use std::mem::size_of;
use std::path::Path;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, GetDIBits, GetObjectW, HBITMAP, HDC, ReleaseDC,
};
use windows::Win32::UI::Shell::SHDefExtractIconW;
use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, HICON, ICONINFO};
use windows::core::{HSTRING, Owned};

use crate::icon::{self, IconImage};

/// The first icon of `exe`, as the shell shows it, at `size_px`×`size_px`.
pub(crate) fn extract(exe: &Path, size_px: u32) -> Option<IconImage> {
    let path = HSTRING::from(exe.as_os_str());
    let mut large = HICON::default();
    // The size of the large icon is the low word of the last argument. The shell picks the best fit from the icon's
    // sizes and scales it if none matches, so there is no need to look at the file ourselves.
    let result = unsafe { SHDefExtractIconW(&path, 0, 0, Some(&mut large), None, size_px) };
    let icon = unsafe { Owned::new(large) };
    if result.is_err() || icon.is_invalid() {
        return None;
    }
    read(*icon, size_px)
}

fn read(icon: HICON, size_px: u32) -> Option<IconImage> {
    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let (color, mask) = unsafe { (Owned::new(info.hbmColor), Owned::new(info.hbmMask)) };
    // Without a color bitmap the icon is black and white and its mask holds both halves. Nothing ships those anymore.
    if color.is_invalid() {
        return None;
    }

    let mut bitmap = BITMAP::default();
    let size = size_of::<BITMAP>() as i32;
    if unsafe { GetObjectW((*color).into(), size, Some((&raw mut bitmap).cast())) } == 0 {
        return None;
    }
    let (width, height) = (
        u32::try_from(bitmap.bmWidth).ok()?,
        u32::try_from(bitmap.bmHeight).ok()?,
    );
    if width == 0 || height == 0 {
        return None;
    }

    let screen = ScreenDc::get()?;
    let mut bgra = pixels(screen.0, *color, width, height)?;
    if !icon::has_alpha(&bgra) {
        icon::alpha_from_mask(&mut bgra, &pixels(screen.0, *mask, width, height)?);
    }
    Some(icon::from_bgra(width, height, bgra, size_px))
}

/// The pixels of `bitmap` as 32-bit BGRA, top row first. Bitmaps with alpha keep it; others get alpha 0.
fn pixels(dc: HDC, bitmap: HBITMAP, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: i32::try_from(width).ok()?,
            // A negative height asks for the top row first.
            biHeight: -i32::try_from(height).ok()?,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let lines = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    (lines == i32::try_from(height).ok()?).then_some(pixels)
}

/// The device context of the screen, released on drop.
struct ScreenDc(HDC);

impl ScreenDc {
    fn get() -> Option<Self> {
        let dc = unsafe { GetDC(None) };
        (!dc.is_invalid()).then_some(Self(dc))
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe { ReleaseDC(None, self.0) };
    }
}
