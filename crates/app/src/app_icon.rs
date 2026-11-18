//! App-global Dock icon overrides. Finder continues to use the shipped bundle icon.
//!
//! Asset contract: CFBundleIconName=Forkarak in Assets.car, with Forkarak.icns
//! for macOS 13+. The six app/*.png exports are fixed renders of the same
//! layered artwork, not live Icon Composer stacks. Never replace the default
//! with one of those renders: nil lets macOS own the native bundle material.
//! On macOS 26+, explicit colors use public named catalog images when available.
//! Their picker previews remain snapshots; only the system owns Dock dynamics.
pub const PREFERENCE_KEY: &str = "appearance.app_icon";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconChoice {
    #[default]
    FollowDefault,
    Charcoal,
    DeepBlue,
    DeepGreen,
    Plum,
    Ivory,
    LightBlue,
}

impl IconChoice {
    pub const ALL: [Self; 7] = [
        Self::FollowDefault,
        Self::Charcoal,
        Self::DeepBlue,
        Self::DeepGreen,
        Self::Plum,
        Self::Ivory,
        Self::LightBlue,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::FollowDefault => "follow_default",
            Self::Charcoal => "charcoal",
            Self::DeepBlue => "deep-blue",
            Self::DeepGreen => "deep-green",
            Self::Plum => "plum",
            Self::Ivory => "ivory",
            Self::LightBlue => "light-blue",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::FollowDefault => "기본값 따르기",
            Self::Charcoal => "차콜",
            Self::DeepBlue => "딥 블루",
            Self::DeepGreen => "딥 그린",
            Self::Plum => "플럼",
            Self::Ivory => "라이트 아이보리",
            Self::LightBlue => "라이트 블루",
        }
    }

    pub fn preview_path(self) -> String {
        format!("app-icons/{}.png", self.id())
    }

    pub fn material_label(self) -> &'static str {
        match self {
            Self::FollowDefault => "시스템 렌더링",
            _ if self.uses_native_artwork() => "네이티브 원본",
            _ => "고정 이미지",
        }
    }

    pub fn uses_native_artwork(self) -> bool {
        #[cfg(target_os = "macos")]
        {
            native_image(self).is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn catalog_name(self) -> Option<&'static str> {
        match self {
            Self::FollowDefault => None,
            Self::Charcoal => Some("Forkarak"),
            Self::DeepBlue => Some("Forkarak-deep-blue"),
            Self::DeepGreen => Some("Forkarak-deep-green"),
            Self::Plum => Some("Forkarak-plum"),
            Self::Ivory => Some("Forkarak-ivory"),
            Self::LightBlue => Some("Forkarak-light-blue"),
        }
    }

    /// Compatibility artwork for platforms without AppKit. On macOS the picker
    /// uses preview_png(), rendering the same NSImage source as the Dock.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn preview_bytes(self) -> &'static [u8] {
        match self {
            Self::FollowDefault => include_bytes!("../../../assets/icons/Forkarak-1024.png"),
            _ => self
                .override_bytes()
                .expect("fixed color has a rendered image"),
        }
    }

    /// A snapshot, not a live Liquid Glass surface. A bundle lookup is independent
    /// of applicationIconImage, which may currently contain a selected override.
    #[cfg(target_os = "macos")]
    pub fn preview_png(self) -> anyhow::Result<std::borrow::Cow<'static, [u8]>> {
        let image = match self {
            Self::FollowDefault => bundled_image()?,
            _ => override_image(self)?.expect("fixed choice has an image"),
        };
        Ok(std::borrow::Cow::Owned(render_preview(&image)?))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn preview_png(self) -> anyhow::Result<std::borrow::Cow<'static, [u8]>> {
        Ok(std::borrow::Cow::Borrowed(self.preview_bytes()))
    }

    /// Fixed compatibility render; apply() prefers a supported named catalog image.
    /// None means clear the runtime override. Never resolve FollowDefault to charcoal:
    /// the installed bundle's catalog/compatibility icon is the authority.
    pub fn override_bytes(self) -> Option<&'static [u8]> {
        Some(match self {
            Self::FollowDefault => return None,
            // One 1024px rendition avoids ICNS representation selection/cache
            // differences between AppKit's setter and an independent thumbnail.
            Self::Charcoal => include_bytes!("../../../assets/icons/app/charcoal.png"),
            Self::DeepBlue => include_bytes!("../../../assets/icons/app/deep-blue.png"),
            Self::DeepGreen => include_bytes!("../../../assets/icons/app/deep-green.png"),
            Self::Plum => include_bytes!("../../../assets/icons/app/plum.png"),
            Self::Ivory => include_bytes!("../../../assets/icons/app/ivory.png"),
            Self::LightBlue => include_bytes!("../../../assets/icons/app/light-blue.png"),
        })
    }
}

#[cfg(target_os = "macos")]
fn native_image(choice: IconChoice) -> Option<objc2::rc::Retained<objc2_app_kit::NSImage>> {
    use objc2_app_kit::NSBundleImageExtension;
    use objc2_foundation::{NSBundle, NSCopying, NSProcessInfo, NSString};
    if NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        < 26
    {
        return None;
    }
    let name = NSString::from_str(choice.catalog_name()?);
    // Public AppKit category; actual compiled names agreed with the asset worker.
    // Copy the cached named image before handing it to the mutable AppKit setter.
    NSBundle::mainBundle()
        .imageForResource(&name)
        .map(|image| image.copy())
}

#[cfg(target_os = "macos")]
fn override_image(
    choice: IconChoice,
) -> anyhow::Result<Option<objc2::rc::Retained<objc2_app_kit::NSImage>>> {
    match native_image(choice) {
        Some(image) => Ok(Some(image)),
        None => choice.override_bytes().map(image_from_bytes).transpose(),
    }
}

#[cfg(target_os = "macos")]
fn image_from_bytes(bytes: &[u8]) -> anyhow::Result<objc2::rc::Retained<objc2_app_kit::NSImage>> {
    use objc2::AnyThread;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSData;
    // NSData copies the bytes; NSImage owns its representations after this returns.
    NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes))
        .ok_or_else(|| anyhow::anyhow!("앱 아이콘 이미지를 읽지 못했습니다"))
}

#[cfg(target_os = "macos")]
fn bundled_image() -> anyhow::Result<objc2::rc::Retained<objc2_app_kit::NSImage>> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSBundle;
    let path = NSBundle::mainBundle().bundlePath();
    if std::path::Path::new(&path.to_string())
        .extension()
        .is_some_and(|extension| extension == "app")
    {
        // Public, read-only API, available before macOS 13 and safe on loader
        // threads. macOS resolves the actual bundle's catalog/fallback itself;
        // no private icon-stack selectors or assumed NSImage image names.
        return Ok(NSWorkspace::sharedWorkspace().iconForFile(&path));
    }
    // A cargo executable has no app bundle icon. Its settings preview still
    // uses the shipped compatibility artwork, never a generic executable icon.
    image_from_bytes(include_bytes!("../../../assets/icons/Forkarak.icns"))
}

#[cfg(target_os = "macos")]
fn render_preview(image: &objc2_app_kit::NSImage) -> anyhow::Result<Vec<u8>> {
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSCalibratedRGBColorSpace, NSColorSpace,
        NSCompositingOperation, NSGraphicsContext,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize};
    // 128 px covers the 56 pt picker at 2x. Each call owns its image and bitmap;
    // the current graphics context is thread-local and restored before return.
    // SAFETY: null planes asks AppKit to allocate its own RGBA8 storage.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(), std::ptr::null_mut(), 128, 128, 8, 4, true,
            false, NSCalibratedRGBColorSpace, 512, 32,
        )
    }.ok_or_else(|| anyhow::anyhow!("앱 아이콘 미리보기 비트맵을 만들지 못했습니다"))?;
    // Retag the empty surface before drawing. GPUI decodes PNG pixels as sRGB;
    // baking generic calibrated-RGB channels would subtly shift the material.
    let bitmap = bitmap
        .bitmapImageRepByRetaggingWithColorSpace(&NSColorSpace::sRGBColorSpace())
        .ok_or_else(|| anyhow::anyhow!("앱 아이콘 미리보기 색상 공간을 만들지 못했습니다"))?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)
        .ok_or_else(|| anyhow::anyhow!("앱 아이콘 미리보기 컨텍스트를 만들지 못했습니다"))?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect_fromRect_operation_fraction(
        NSRect::new(NSPoint::ZERO, NSSize::new(128., 128.)),
        NSRect::ZERO,
        NSCompositingOperation::Copy,
        1.,
    );
    NSGraphicsContext::restoreGraphicsState_class();
    // SAFETY: an empty properties dictionary has no incorrectly typed values.
    unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .map(|data| data.to_vec())
    .ok_or_else(|| anyhow::anyhow!("앱 아이콘 미리보기를 인코딩하지 못했습니다"))
}

#[cfg(target_os = "macos")]
pub fn apply(choice: IconChoice) -> anyhow::Result<()> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let main = MainThreadMarker::new()
        .ok_or_else(|| anyhow::anyhow!("앱 아이콘은 메인 스레드에서 변경해야 합니다"))?;
    let image = override_image(choice)?;
    let application = NSApplication::sharedApplication(main);
    // SAFETY: main-thread marker above; image is owned until after the strong setter.
    // AppKit explicitly documents nil as restoring the original bundled Dock icon:
    // https://developer.apple.com/documentation/appkit/nsapplication/applicationiconimage
    unsafe { application.setApplicationIconImage(image.as_deref()) };
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn apply(_choice: IconChoice) -> anyhow::Result<()> {
    Ok(())
}
