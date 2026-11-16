//! File identity is independent of Git status and never requires filesystem access.

use crate::theme::Palette;
use gpui_kit::{Styled, component::Icon, px};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileKind {
    Java,
    Kotlin,
    JavaScript,
    TypeScript,
    React,
    Vue,
    Svelte,
    Html,
    Css,
    Scss,
    Json,
    Yaml,
    Xml,
    Maven,
    Gradle,
    Rust,
    Cargo,
    Markdown,
    GitIgnore,
    Generic,
}

impl FileKind {
    fn for_path(path: &Path) -> Self {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Self::Generic;
        };
        let name = name.to_ascii_lowercase();
        // Build manifests and compound suffixes take precedence over extensions.
        match name.as_str() {
            "pom.xml" => return Self::Maven,
            "cargo.toml" | "cargo.lock" => return Self::Cargo,
            "gradlew" | "gradlew.bat" | "gradle.properties" | "gradle-wrapper.properties" => {
                return Self::Gradle;
            }
            ".gitignore" => return Self::GitIgnore,
            _ if name.ends_with(".gradle.kts") => return Self::Gradle,
            _ => {}
        }
        match Path::new(&name).extension().and_then(|ext| ext.to_str()) {
            Some("java") => Self::Java,
            Some("kt" | "kts") => Self::Kotlin,
            Some("js" | "mjs" | "cjs") => Self::JavaScript,
            Some("ts" | "mts" | "cts") => Self::TypeScript,
            Some("jsx" | "tsx") => Self::React,
            Some("vue") => Self::Vue,
            Some("svelte") => Self::Svelte,
            Some("html" | "htm") => Self::Html,
            Some("css") => Self::Css,
            Some("scss" | "sass") => Self::Scss,
            Some("json" | "jsonc" | "json5") => Self::Json,
            Some("yaml" | "yml") => Self::Yaml,
            Some("xml") => Self::Xml,
            Some("gradle") => Self::Gradle,
            Some("rs") => Self::Rust,
            Some("md" | "markdown" | "mdx") => Self::Markdown,
            _ => Self::Generic,
        }
    }

    fn svg(self) -> &'static [u8] {
        match self {
            Self::Java => include_bytes!("../../../../assets/icons/files/java.svg"),
            Self::Kotlin => include_bytes!("../../../../assets/icons/files/kotlin.svg"),
            Self::JavaScript => include_bytes!("../../../../assets/icons/files/javascript.svg"),
            Self::TypeScript => include_bytes!("../../../../assets/icons/files/typescript.svg"),
            Self::React => include_bytes!("../../../../assets/icons/files/react.svg"),
            Self::Vue => include_bytes!("../../../../assets/icons/files/vue.svg"),
            Self::Svelte => include_bytes!("../../../../assets/icons/files/svelte.svg"),
            Self::Html => include_bytes!("../../../../assets/icons/files/html.svg"),
            Self::Css => include_bytes!("../../../../assets/icons/files/css.svg"),
            Self::Scss => include_bytes!("../../../../assets/icons/files/scss.svg"),
            Self::Json => include_bytes!("../../../../assets/icons/files/json.svg"),
            Self::Yaml => include_bytes!("../../../../assets/icons/files/yaml.svg"),
            Self::Xml => include_bytes!("../../../../assets/icons/files/xml.svg"),
            Self::Maven => include_bytes!("../../../../assets/icons/files/maven.svg"),
            Self::Gradle => include_bytes!("../../../../assets/icons/files/gradle.svg"),
            Self::Rust => include_bytes!("../../../../assets/icons/files/rust.svg"),
            Self::Cargo => include_bytes!("../../../../assets/icons/files/cargo.svg"),
            Self::Markdown => include_bytes!("../../../../assets/icons/files/markdown.svg"),
            Self::GitIgnore => include_bytes!("../../../../assets/icons/files/gitignore.svg"),
            Self::Generic => include_bytes!("../../../../assets/icons/files/file.svg"),
        }
    }

    fn color(self) -> u32 {
        // Existing palette tokens supply contrasting light/dark variants.
        match self {
            Self::Java | Self::Svelte | Self::Html | Self::Scss | Self::Maven => 0xe5a0a5,
            Self::Kotlin
            | Self::TypeScript
            | Self::React
            | Self::Css
            | Self::Xml
            | Self::Markdown => 0x7fb8e2,
            Self::Vue | Self::Gradle => 0x83c4a1,
            Self::JavaScript | Self::Json | Self::Rust | Self::Cargo => 0xe4bb79,
            Self::Yaml | Self::GitIgnore | Self::Generic => 0xa4b0be,
        }
    }
}

/// Embedded, theme-aware SVG usable both as a row child and in `Button::icon`.
pub(super) fn icon_for_path(path: &Path, palette: Palette) -> Icon {
    let kind = FileKind::for_path(path);
    Icon::default()
        .data(kind.svg())
        .size(px(14.))
        .text_color(palette.rgb(kind.color()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions() {
        for (kind, extensions) in [
            (FileKind::Java, &["java"][..]),
            (FileKind::Kotlin, &["kt", "kts"]),
            (FileKind::JavaScript, &["js", "mjs", "cjs"]),
            (FileKind::TypeScript, &["ts", "mts", "cts"]),
            (FileKind::React, &["jsx", "tsx"]),
            (FileKind::Vue, &["vue"]),
            (FileKind::Svelte, &["svelte"]),
            (FileKind::Html, &["html", "htm"]),
            (FileKind::Css, &["css"]),
            (FileKind::Scss, &["scss", "sass"]),
            (FileKind::Json, &["json", "jsonc", "json5"]),
            (FileKind::Yaml, &["yaml", "yml"]),
            (FileKind::Xml, &["xml"]),
            (FileKind::Gradle, &["gradle"]),
            (FileKind::Rust, &["rs"]),
            (FileKind::Markdown, &["md", "markdown", "mdx"]),
        ] {
            for extension in extensions {
                let path = format!("src/file.{extension}");
                assert_eq!(FileKind::for_path(Path::new(&path)), kind, "{path}");
            }
        }
    }

    #[test]
    fn special_filenames_precede_extensions_and_use_only_the_basename() {
        for (path, expected) in [
            ("project/pom.xml", FileKind::Maven),
            ("project/Cargo.toml", FileKind::Cargo),
            ("project/Cargo.lock", FileKind::Cargo),
            ("build.gradle", FileKind::Gradle),
            ("settings.gradle", FileKind::Gradle),
            ("build.gradle.kts", FileKind::Gradle),
            ("settings.gradle.kts", FileKind::Gradle),
            ("init.gradle.kts", FileKind::Gradle),
            ("gradle.properties", FileKind::Gradle),
            ("gradle/wrapper/gradle-wrapper.properties", FileKind::Gradle),
            ("gradlew", FileKind::Gradle),
            ("gradlew.bat", FileKind::Gradle),
            ("project/.gitignore", FileKind::GitIgnore),
            ("script.kts", FileKind::Kotlin),
            ("pom.xml/other.xml", FileKind::Xml),
            ("Cargo.toml/other.toml", FileKind::Generic),
            ("build.gradle.kts/script.kts", FileKind::Kotlin),
            ("types.d.ts", FileKind::TypeScript),
            ("pom.xml.bak", FileKind::Generic),
        ] {
            assert_eq!(FileKind::for_path(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn case_insensitive_names_and_unknown_paths() {
        for (path, expected) in [
            ("Main.JAVA", FileKind::Java),
            ("Component.TsX", FileKind::React),
            ("POM.XML", FileKind::Maven),
            ("CARGO.LOCK", FileKind::Cargo),
            ("SETTINGS.GRADLE.KTS", FileKind::Gradle),
            (".GITIGNORE", FileKind::GitIgnore),
            ("README", FileKind::Generic),
            (".env", FileKind::Generic),
            ("archive.unknown", FileKind::Generic),
            ("file.", FileKind::Generic),
            ("", FileKind::Generic),
            ("/", FileKind::Generic),
            ("src/한글.kt", FileKind::Kotlin),
        ] {
            assert_eq!(FileKind::for_path(Path::new(path)), expected, "{path}");
        }
    }
}
