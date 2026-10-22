//! App icons: the icon Finder shows for an app's bundle (or executable), drawn at the size asked for.

/// Turns premultiplied RGBA pixels, as AppKit draws them, into straight alpha. Fully transparent pixels become
/// transparent black.
pub(crate) fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            *pixel = [0; 4];
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use self::sys::bundle_icon;

#[cfg(target_os = "macos")]
mod sys {
    use std::path::Path;

    use objc2::AnyThread;
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{
        NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace, NSGraphicsContext, NSWorkspace,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    use super::unpremultiply;
    use crate::IconImage;

    /// The icon of the bundle or file at `path`, drawn at `size_px` square pixels (1–1024, checked by `app_icon`).
    /// `None` if there is no such file. Runs off the main thread, drawing into a bitmap of its own.
    pub(crate) fn bundle_icon(path: &Path, size_px: u32) -> Option<IconImage> {
        if !path.exists() {
            return None;
        }
        let path = NSString::from_str(path.to_str()?);
        autoreleasepool(|_| {
            let image = NSWorkspace::sharedWorkspace().iconForFile(&path);
            let size = size_px as isize;
            // SAFETY: a null plane pointer makes the rep allocate its own pixels, of the size the other arguments give.
            let rep = unsafe {
                NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                    NSBitmapImageRep::alloc(),
                    std::ptr::null_mut(),
                    size,
                    size,
                    8,
                    4,
                    true,
                    false,
                    NSDeviceRGBColorSpace,
                    size * 4,
                    32,
                )
            }?;
            let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
            NSGraphicsContext::saveGraphicsState_class();
            NSGraphicsContext::setCurrentContext(Some(&context));
            let rect = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(f64::from(size_px), f64::from(size_px)),
            );
            image.drawInRect_fromRect_operation_fraction(rect, NSRect::ZERO, NSCompositingOperation::Copy, 1.0);
            context.flushGraphics();
            NSGraphicsContext::restoreGraphicsState_class();

            let row = usize::try_from(rep.bytesPerRow()).ok()?;
            let data = rep.bitmapData();
            if data.is_null() || row < size_px as usize * 4 {
                return None;
            }
            let side = size_px as usize;
            let mut rgba = Vec::with_capacity(side * side * 4);
            for y in 0..side {
                // SAFETY: the rep owns `row` bytes for each of its `side` rows, and lives until the pool drains.
                let line = unsafe { std::slice::from_raw_parts(data.add(y * row), side * 4) };
                rgba.extend_from_slice(line);
            }
            unpremultiply(&mut rgba);
            Some(IconImage {
                width: size_px,
                height: size_px,
                rgba,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_pixels_become_straight_alpha() {
        let mut pixels = [
            [255, 128, 0, 255], // opaque: as it is
            [64, 32, 0, 128],   // half see-through: the colour of the opaque one, roughly
            [1, 2, 3, 0],       // see-through: nothing left of it
            [3, 2, 1, 5],       // rounds to the nearest
            [9, 0, 0, 3],       // a colour above alpha (bad input) is capped
        ]
        .concat();
        unpremultiply(&mut pixels);
        assert_eq!(
            pixels,
            [
                [255, 128, 0, 255],
                [128, 64, 0, 128],
                [0, 0, 0, 0],
                [153, 102, 51, 5],
                [255, 0, 0, 3]
            ]
            .concat()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn icons_of_bundles_and_executables() {
        use std::path::Path;

        let finder = Path::new("/System/Library/CoreServices/Finder.app");
        for size in [32, 64] {
            let icon = crate::app_icon(finder, size).expect("Finder has an icon");
            assert_eq!(
                (icon.width, icon.height, icon.rgba.len()),
                (size, size, (size * size * 4) as usize)
            );
            let pixels = icon.rgba.as_chunks::<4>().0;
            let opaque = pixels.iter().filter(|p| p[3] == 255).count();
            assert!(opaque > pixels.len() / 4, "{opaque} of {} opaque", pixels.len());
        }
        // A plain executable gets the generic one.
        assert!(crate::app_icon(Path::new("/bin/ls"), 16).is_some());
        assert_eq!(crate::app_icon(Path::new("/no/such/app.app"), 32), None);
        assert_eq!(crate::app_icon(finder, 0), None);
        assert_eq!(crate::app_icon(finder, 1025), None);
    }
}
