//! Main-thread AppKit smoke probe. Does not activate or create a window.
//! cargo run -p pokarak --example app_icon_probe -- /tmp/icon-readback
#[allow(dead_code)]
#[path = "../src/app_icon.rs"]
mod app_icon;

#[cfg(target_os = "macos")]
fn pixels(image: &objc2_app_kit::NSImage) -> Vec<u8> {
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapImageRep, NSCalibratedRGBColorSpace, NSCompositingOperation, NSGraphicsContext,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    // SAFETY: null planes asks AppKit to allocate its own owned storage.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(), std::ptr::null_mut(), 1024, 1024, 8, 4, true, false,
            NSCalibratedRGBColorSpace, 4096, 32,
        ).unwrap()
    };
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap).unwrap();
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect_fromRect_operation_fraction(
        NSRect::new(NSPoint::new(0., 0.), NSSize::new(1024., 1024.)),
        NSRect::ZERO,
        NSCompositingOperation::Copy,
        1.,
    );
    NSGraphicsContext::restoreGraphicsState_class();
    // SAFETY: owned nonplanar 1024x1024 RGBA8 storage stays alive during the copy.
    unsafe { std::slice::from_raw_parts(bitmap.bitmapData(), 4096 * 1024) }.to_vec()
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use app_icon::IconChoice;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let destination = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    if let Some(path) = &destination {
        std::fs::create_dir_all(path)?;
    }
    let main = MainThreadMarker::new().expect("probe starts on the process main thread");
    let app = NSApplication::sharedApplication(main);
    app_icon::apply(IconChoice::FollowDefault)?;
    let original = app.applicationIconImage().unwrap();
    if let Some(path) = std::env::args_os().nth(2) {
        let data = NSData::with_bytes(&std::fs::read(path)?);
        let shipped = NSImage::initWithData(NSImage::alloc(), &data).unwrap();
        let original_pixels = pixels(&original);
        let difference: usize = original_pixels
            .iter()
            .zip(pixels(&shipped))
            .map(|(&a, b)| usize::from(a.abs_diff(b)))
            .sum();
        let mean = difference as f64 / (4096 * 1024) as f64;
        if let Some(path) = &destination {
            std::fs::write(
                path.join("bundled-default.tiff"),
                original.TIFFRepresentation().unwrap().to_vec(),
            )?;
            std::fs::write(
                path.join("bundled-expected.tiff"),
                shipped.TIFFRepresentation().unwrap().to_vec(),
            )?;
        }
        println!("Bundled default native render versus source: mean RGBA error {mean:.4}/255");
        // The system's bundle-icon renderer adds its own framing/shadow. Verify
        // source identity against every preset rather than demanding raw equality.
        for choice in IconChoice::ALL {
            let Some(bytes) = choice.override_bytes() else {
                continue;
            };
            let alternative =
                NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes)).unwrap();
            let other: usize = original_pixels
                .iter()
                .zip(pixels(&alternative))
                .map(|(&a, b)| usize::from(a.abs_diff(b)))
                .sum();
            let other_mean = other as f64 / (4096 * 1024) as f64;
            anyhow::ensure!(
                mean <= other_mean + 0.01,
                "FollowDefault is closer to {} ({other_mean}) than shipped source ({mean})",
                choice.id()
            );
        }
        anyhow::ensure!(mean < 12., "Unrecognized bundled icon render: {mean}");
        println!(
            "Verified FollowDefault uses this bundle's shipped icon (mean RGBA error {mean:.4}/255)"
        );
    }
    for choice in IconChoice::ALL {
        app_icon::apply(choice)?;
        let image = app.applicationIconImage().expect("AppKit icon readback");
        let actual = image.TIFFRepresentation().expect("TIFF readback").to_vec();
        let expected_image = if let Some(bytes) = choice.override_bytes() {
            NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes)).unwrap()
        } else {
            original.clone()
        };
        let expected = expected_image.TIFFRepresentation().unwrap().to_vec();
        let (actual_pixels, expected_pixels) = (pixels(&image), pixels(&expected_image));
        let difference: usize = actual_pixels
            .iter()
            .zip(&expected_pixels)
            .map(|(&a, &b)| usize::from(a.abs_diff(b)))
            .sum();
        let mean_difference = difference as f64 / actual_pixels.len() as f64;
        let preview = NSImage::initWithData(
            NSImage::alloc(),
            &NSData::with_bytes(choice.preview_bytes()),
        );
        anyhow::ensure!(preview.is_some(), "Undecodable preview: {}", choice.id());
        if let Some(path) = &destination {
            std::fs::write(path.join(format!("{}.tiff", choice.id())), &actual)?;
            std::fs::write(
                path.join(format!("{}-expected.tiff", choice.id())),
                &expected,
            )?;
        }
        anyhow::ensure!(
            mean_difference < 2.,
            "AppKit pixel readback mismatch: {} mean channel error {}",
            choice.id(),
            mean_difference
        );
        println!(
            "Verified main-thread icon/readback and preview: {} (mean RGBA error {:.4}/255)",
            choice.id(),
            mean_difference
        );
    }
    // Source NSData/NSImage locals in apply have already dropped before readback.
    app_icon::apply(IconChoice::FollowDefault)?;
    anyhow::ensure!(
        pixels(&app.applicationIconImage().unwrap()) == pixels(&original),
        "nil did not restore the original application icon"
    );
    let off_main = std::thread::spawn(|| app_icon::apply(IconChoice::Plum).is_err())
        .join()
        .unwrap();
    anyhow::ensure!(off_main, "off-main-thread icon mutation was accepted");
    println!("Verified nil restores original and off-main-thread requests are rejected");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {}
