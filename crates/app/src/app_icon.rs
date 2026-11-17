//! App-global Dock icon overrides. Finder continues to use the shipped bundle icon.
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

    pub fn preview_bytes(self) -> &'static [u8] {
        match self {
            Self::FollowDefault => include_bytes!("../../../assets/icons/Forkarak-1024.png"),
            Self::Charcoal => {
                include_bytes!("../../../assets/icons/app/charcoal.iconset/icon_128x128.png")
            }
            Self::DeepBlue => {
                include_bytes!("../../../assets/icons/app/deep-blue.iconset/icon_128x128.png")
            }
            Self::DeepGreen => {
                include_bytes!("../../../assets/icons/app/deep-green.iconset/icon_128x128.png")
            }
            Self::Plum => include_bytes!("../../../assets/icons/app/plum.iconset/icon_128x128.png"),
            Self::Ivory => {
                include_bytes!("../../../assets/icons/app/ivory.iconset/icon_128x128.png")
            }
            Self::LightBlue => {
                include_bytes!("../../../assets/icons/app/light-blue.iconset/icon_128x128.png")
            }
        }
    }

    /// None means clear the runtime override. Never resolve FollowDefault to charcoal:
    /// a future release's CFBundleIconFile is the authority for that choice.
    pub fn override_bytes(self) -> Option<&'static [u8]> {
        Some(match self {
            Self::FollowDefault => return None,
            Self::Charcoal => include_bytes!("../../../assets/icons/app/charcoal.icns"),
            Self::DeepBlue => include_bytes!("../../../assets/icons/app/deep-blue.icns"),
            Self::DeepGreen => include_bytes!("../../../assets/icons/app/deep-green.icns"),
            Self::Plum => include_bytes!("../../../assets/icons/app/plum.icns"),
            Self::Ivory => include_bytes!("../../../assets/icons/app/ivory.icns"),
            Self::LightBlue => include_bytes!("../../../assets/icons/app/light-blue.icns"),
        })
    }
}

#[cfg(target_os = "macos")]
pub fn apply(choice: IconChoice) -> anyhow::Result<()> {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let main = MainThreadMarker::new()
        .ok_or_else(|| anyhow::anyhow!("앱 아이콘은 메인 스레드에서 변경해야 합니다"))?;
    let image = choice
        .override_bytes()
        .map(|bytes| {
            // NSData copies the embedded bytes; NSImage and AppKit retain their objects.
            // No borrowed Objective-C pointers or filesystem/PATH dependencies survive.
            let data = NSData::with_bytes(bytes);
            NSImage::initWithData(NSImage::alloc(), &data)
                .ok_or_else(|| anyhow::anyhow!("앱 아이콘 이미지를 읽지 못했습니다"))
        })
        .transpose()?;
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
