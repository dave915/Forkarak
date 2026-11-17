//! Filename-only selection also works for deleted files and historical tree entries.
use crate::theme::ui_px;
use gpui_kit::component::{ActiveTheme, button::Button};
use gpui_kit::{App, ElementId, Img, ParentElement, SharedString, Styled, div, img};
use serde::Deserialize;
use std::{collections::HashMap, path::Path, sync::LazyLock};

#[derive(Deserialize)]
struct Associations {
    default: String,
    names: HashMap<String, String>,
    suffixes: HashMap<String, String>,
    light: HashMap<String, String>,
}

static THEME: LazyLock<Associations> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../../assets/icons/vscode/associations.json"
    ))
    .expect("bundled VSCode Icons associations")
});

impl Associations {
    fn artwork(&self, path: &Path, light: bool) -> &str {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let icon = self
            .names
            .get(&name)
            .or_else(|| {
                // Longest compound suffix wins (e.g. d.ts before ts).
                name.match_indices('.')
                    .find_map(|(index, _)| self.suffixes.get(&name[index + 1..]))
            })
            .unwrap_or(&self.default);
        if light {
            self.light.get(icon).unwrap_or(icon)
        } else {
            icon
        }
    }
}

pub(super) fn icon_for_path(path: &Path, cx: &App) -> Img {
    let name = THEME.artwork(path, !cx.theme().is_dark());
    img(format!("vscode-icons/{name}"))
        .size(ui_px(16.))
        .flex_none()
}

pub(super) fn file_button(
    id: impl Into<ElementId>,
    path: &Path,
    label: impl Into<SharedString>,
    cx: &App,
) -> Button {
    let label = label.into();
    Button::new(id)
        .accessibility_label(label.clone())
        .child(icon_for_path(path, cx))
        .child(div().min_w_0().text_ellipsis().child(label))
}

pub(super) fn changed_file_button(
    id: impl Into<ElementId>,
    path: &Path,
    status: &'static str,
    display_path: String,
    cx: &App,
) -> Button {
    let label = format!("{status} {display_path}");
    let row = div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap_1()
        .text_left()
        .child(icon_for_path(path, cx))
        .child(div().w(ui_px(14.)).flex_none().child(status))
        .child(div().flex_1().min_w_0().text_ellipsis().child(display_path));
    Button::new(id)
        .accessibility_label(label.clone())
        .tooltip(label)
        .child(row)
}

#[cfg(test)]
mod tests {
    use super::THEME;
    use std::path::Path;

    #[test]
    fn exact_names_then_compound_suffixes_then_fallback() {
        for (path, asset) in [
            ("src/Main.JAVA", "file_type_java.svg"),
            ("src/한글.kts", "file_type_kotlin.svg"),
            ("package.json", "file_type_npm.svg"),
            ("src/types.d.ts", "file_type_typescriptdef.svg"),
            ("view.tsx", "file_type_reactts.svg"),
            ("CARGO.TOML", "file_type_cargo.svg"),
            ("settings.GRADLE.KTS", "file_type_gradle.svg"),
            ("POM.XML", "file_type_maven.svg"),
            ("Dockerfile", "file_type_docker.svg"),
            (".gitignore", "file_type_git.svg"),
            ("main.rs/unknown.xyz-quux", "default_file.svg"),
            ("/", "default_file.svg"),
        ] {
            assert_eq!(THEME.artwork(Path::new(path), false), asset, "{path}");
        }
    }

    #[test]
    fn light_theme_uses_original_variants_and_all_referenced_assets_exist() {
        assert_eq!(
            THEME.artwork(Path::new("main.rs"), true),
            "file_type_light_rust.svg"
        );
        assert_eq!(
            THEME.artwork(Path::new("unknown.zzzzz"), true),
            "default_file.svg"
        );
        assert_eq!(
            THEME.artwork(Path::new("main.ts"), true),
            THEME.artwork(Path::new("main.ts"), false)
        );
        let artwork: std::collections::HashSet<_> = THEME
            .names
            .values()
            .chain(THEME.suffixes.values())
            .chain(THEME.light.values())
            .chain([&THEME.default])
            .collect();
        for name in artwork {
            assert!(crate::assets::contains_file_artwork(name), "missing {name}");
        }
    }
}
