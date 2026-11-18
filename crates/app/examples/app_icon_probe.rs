//! Main-thread AppKit smoke probe. Does not activate or create a window.
//! cargo run -p pokarak --example app_icon_probe -- /tmp/icon-readback
//! In an isolated bundle, add --catalog to diagnose public named catalog loading.
#[allow(dead_code)]
#[path = "../src/app_icon.rs"]
mod app_icon;

#[cfg(target_os = "macos")]
fn pixels(image: &objc2_app_kit::NSImage) -> Vec<u8> {
    pixels_at(image, 1024)
}

#[cfg(target_os = "macos")]
fn pixels_at(image: &objc2_app_kit::NSImage, side: usize) -> Vec<u8> {
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapImageRep, NSCalibratedRGBColorSpace, NSColorSpace, NSCompositingOperation,
        NSGraphicsContext,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    // SAFETY: null planes asks AppKit to allocate its own owned storage.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(), std::ptr::null_mut(), side as isize, side as isize,
            8, 4, true, false, NSCalibratedRGBColorSpace, (side * 4) as isize, 32,
        ).unwrap()
    };
    let bitmap = bitmap
        .bitmapImageRepByRetaggingWithColorSpace(&NSColorSpace::sRGBColorSpace())
        .unwrap();
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap).unwrap();
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect_fromRect_operation_fraction(
        NSRect::new(NSPoint::new(0., 0.), NSSize::new(side as f64, side as f64)),
        NSRect::ZERO,
        NSCompositingOperation::Copy,
        1.,
    );
    NSGraphicsContext::restoreGraphicsState_class();
    // SAFETY: owned nonplanar side x side RGBA8 storage stays alive during copy.
    unsafe { std::slice::from_raw_parts(bitmap.bitmapData(), side * side * 4) }.to_vec()
}

#[cfg(target_os = "macos")]
fn expected_override(choice: app_icon::IconChoice) -> objc2::rc::Retained<objc2_app_kit::NSImage> {
    use objc2::AnyThread;
    use objc2_app_kit::{NSBundleImageExtension, NSImage};
    use objc2_foundation::{NSBundle, NSCopying, NSData, NSString};
    if choice.uses_native_artwork() {
        NSBundle::mainBundle()
            .imageForResource(&NSString::from_str(choice.catalog_name().unwrap()))
            .unwrap()
            .copy()
    } else {
        NSImage::initWithData(
            NSImage::alloc(),
            &NSData::with_bytes(choice.override_bytes().unwrap()),
        )
        .unwrap()
    }
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use app_icon::IconChoice;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSImage};
    use objc2_foundation::NSData;

    let destination = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    if let Some(path) = &destination {
        std::fs::create_dir_all(path)?;
    }
    let main = MainThreadMarker::new().expect("probe starts on the process main thread");
    let app = NSApplication::sharedApplication(main);
    // This isolated helper must not create a window, activate, or get a Dock tile.
    if app.activationPolicy() != NSApplicationActivationPolicy::Prohibited {
        app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    }
    anyhow::ensure!(
        app.activationPolicy() == NSApplicationActivationPolicy::Prohibited,
        "probe must remain prohibited from activating or creating a Dock tile"
    );
    app_icon::apply(IconChoice::FollowDefault)?;
    let original = app.applicationIconImage().unwrap();
    let original_pixels = pixels(&original);
    let original_snapshot =
        NSImage::initWithData(NSImage::alloc(), &original.TIFFRepresentation().unwrap()).unwrap();
    let default_preview = IconChoice::FollowDefault.preview_png()?.into_owned();
    let default_preview_image =
        NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&default_preview)).unwrap();
    if objc2_foundation::NSBundle::mainBundle()
        .bundlePath()
        .to_string()
        .ends_with(".app")
    {
        let a = pixels_at(&default_preview_image, 128);
        let b = pixels_at(&original, 128);
        let mean = a
            .iter()
            .zip(&b)
            .map(|(&a, &b)| usize::from(a.abs_diff(b)))
            .sum::<usize>() as f64
            / a.len() as f64;
        anyhow::ensure!(
            mean < 2.,
            "Default preview does not match actual bundle icon: {mean}"
        );
        println!("Verified default preview matches actual bundle icon (128px mean RGBA error {mean:.4}/255)");
    }
    if let Some(path) = &destination {
        std::fs::write(path.join("bundle-preview.png"), &default_preview)?;
    }
    if let Some(path) = std::env::args_os().nth(2).filter(|arg| arg != "--catalog") {
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
    if std::env::args_os().any(|arg| arg == "--catalog") {
        use objc2_app_kit::NSBundleImageExtension;
        use objc2_foundation::{NSBundle, NSCopying, NSString};
        let mut loaded = 0;
        for name in [
            "Forkarak",
            "Forkarak-deep-blue",
            "Forkarak-deep-green",
            "Forkarak-plum",
            "Forkarak-ivory",
            "Forkarak-light-blue",
        ] {
            let key = NSString::from_str(name);
            let named = NSImage::imageNamed(&key);
            let resource = NSBundle::mainBundle().imageForResource(&key);
            println!(
                "Public catalog lookup {name}: NSImage.imageNamed={}, NSBundle.imageForResource={}",
                named.is_some(),
                resource.is_some()
            );
            if let Some(image) = named.or(resource) {
                let image = image.copy();
                loaded += 1;
                // SAFETY: main-thread probe; source is retained through readback.
                unsafe {
                    app.setApplicationIconImage(Some(&image));
                }
                let actual = app.applicationIconImage().unwrap();
                let expected = pixels_at(&image, 128);
                let readback = pixels_at(&actual, 128);
                let mean = expected
                    .iter()
                    .zip(&readback)
                    .map(|(&a, &b)| usize::from(a.abs_diff(b)))
                    .sum::<usize>() as f64
                    / expected.len() as f64;
                anyhow::ensure!(mean < 2., "Named image readback mismatch: {name} ({mean})");
                if let Some(path) = &destination {
                    std::fs::write(
                        path.join(format!("catalog-{name}.tiff")),
                        actual.TIFFRepresentation().unwrap().to_vec(),
                    )?;
                }
                println!(
                    "Verified public named image selection/readback: {name} (mean {mean:.4}/255)"
                );
            }
        }
        app_icon::apply(IconChoice::FollowDefault)?;
        let reset = pixels(&app.applicationIconImage().unwrap());
        let expected = original_pixels.clone();
        let mean = reset
            .iter()
            .zip(&expected)
            .map(|(&a, &b)| usize::from(a.abs_diff(b)))
            .sum::<usize>() as f64
            / reset.len() as f64;
        println!("Catalog nil reset mean RGBA error {mean:.4}/255");
        anyhow::ensure!(
            mean < 2.,
            "Catalog diagnostic did not restore bundle default with nil ({mean})"
        );
        println!("Public named icon images loaded: {loaded}/6; nil restored native bundle default");
        anyhow::ensure!(
            loaded == 6,
            "Compiled catalog did not expose all six public named images"
        );
    }
    for choice in IconChoice::ALL {
        app_icon::apply(choice)?;
        let image = app.applicationIconImage().expect("AppKit icon readback");
        let preview_png = choice.preview_png()?;
        let preview = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&preview_png))
            .expect("decodable picker preview");
        if choice.override_bytes().is_some() {
            let preview_pixels = pixels_at(&preview, 128);
            let dock_pixels = pixels_at(&image, 128);
            let fresh = expected_override(choice);
            let fresh_pixels = pixels_at(&fresh, 128);
            let source_mean = preview_pixels
                .iter()
                .zip(&fresh_pixels)
                .map(|(&a, &b)| usize::from(a.abs_diff(b)))
                .sum::<usize>() as f64
                / preview_pixels.len() as f64;
            anyhow::ensure!(
                source_mean < 0.5,
                "Picker preview source mismatch: {} ({source_mean})",
                choice.id()
            );
            let mean = preview_pixels
                .iter()
                .zip(&dock_pixels)
                .map(|(&a, &b)| usize::from(a.abs_diff(b)))
                .sum::<usize>() as f64
                / preview_pixels.len() as f64;
            // PNG's straight-alpha/color-space round trip can round channels.
            anyhow::ensure!(
                mean < 2.,
                "Picker preview differs from selected Dock image: {} mean error {mean}",
                choice.id(),
            );
            println!(
                "Verified picker/Dock source: {} · {} (128px mean RGBA error {mean:.4}/255)",
                choice.id(),
                choice.material_label(),
            );
        }
        // Loading the default thumbnail while an override is selected must neither
        // show that override nor temporarily reset the runtime Dock selection.
        let before_preview = pixels_at(&image, 128);
        let default_now = IconChoice::FollowDefault.preview_png()?;
        let default_now =
            NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&default_now)).unwrap();
        anyhow::ensure!(
            pixels_at(&default_now, 128) == pixels_at(&default_preview_image, 128),
            "Default thumbnail picked up {} override",
            choice.id()
        );
        anyhow::ensure!(
            pixels_at(&app.applicationIconImage().unwrap(), 128) == before_preview,
            "Loading a thumbnail mutated the Dock selection"
        );
        let actual = image.TIFFRepresentation().expect("TIFF readback").to_vec();
        let expected_image = if choice.override_bytes().is_some() {
            expected_override(choice)
        } else {
            original_snapshot.clone()
        };
        let expected = expected_image.TIFFRepresentation().unwrap().to_vec();
        // Compare at Dock/picker scale. Native NSImage reps cache per draw size;
        // a 1024px enlargement after a 128px draw measures that cache policy.
        let (actual_pixels, expected_pixels) =
            (pixels_at(&image, 128), pixels_at(&expected_image, 128));
        let difference: usize = actual_pixels
            .iter()
            .zip(&expected_pixels)
            .map(|(&a, &b)| usize::from(a.abs_diff(b)))
            .sum();
        let mean_difference = difference as f64 / actual_pixels.len() as f64;
        if let Some(path) = &destination {
            std::fs::write(
                path.join(format!("{}-preview.png", choice.id())),
                &preview_png,
            )?;
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
        pixels(&app.applicationIconImage().unwrap()) == original_pixels,
        "nil did not restore the original application icon"
    );
    let off_main = std::thread::spawn(|| app_icon::apply(IconChoice::Plum).is_err())
        .join()
        .unwrap();
    anyhow::ensure!(off_main, "off-main-thread icon mutation was accepted");
    std::thread::spawn(|| -> anyhow::Result<()> {
        for choice in IconChoice::ALL {
            anyhow::ensure!(
                !choice.preview_png()?.is_empty(),
                "Background asset loader returned no preview: {}",
                choice.id()
            );
        }
        Ok(())
    })
    .join()
    .unwrap()?;
    println!("Verified nil restores original and off-main-thread requests are rejected");
    println!("Verified 128px picker/Dock pixels, independent bundle thumbnail, background loading");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {}
