use gpui_kit::{AssetSource, SharedString};
use rust_embed::RustEmbed;
use std::borrow::Cow;

const PREFIX: &str = "vscode-icons/";

#[derive(RustEmbed)]
#[folder = "../../assets/icons/vscode/artwork/"]
struct FileArtwork;

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        match path.strip_prefix(PREFIX) {
            Some(name) => Ok(FileArtwork::get(name).map(|asset| asset.data)),
            None => gpui_kit::assets::AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::AllAssets.list(path)?;
        if PREFIX.starts_with(path) || path.starts_with(PREFIX) {
            paths.extend(
                FileArtwork::iter()
                    .map(|name| format!("{PREFIX}{name}"))
                    .filter(|name| name.starts_with(path))
                    .map(Into::into),
            );
        }
        Ok(paths)
    }
}

#[cfg(test)]
pub(crate) fn contains_file_artwork(name: &str) -> bool {
    FileArtwork::get(name).is_some()
}
