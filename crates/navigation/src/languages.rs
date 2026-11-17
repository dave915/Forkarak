//! Language identification and honest server distribution catalog. No discovery executes tools.
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LanguageSupport {
    pub language: String,
    pub name: String,
    pub package: Option<String>,
    pub license: String,
    pub source_url: String,
    pub limitations: Vec<String>,
}

pub fn language_for_path(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    if name == ".env" || name.starts_with(".env.") {
        return Some("dotenv");
    }
    if matches!(name, "Makefile" | "makefile" | "GNUmakefile") {
        return Some("makefile");
    }
    if name == "Dockerfile" || name.starts_with("Dockerfile.") || name.ends_with(".Dockerfile") {
        return Some("dockerfile");
    }
    if matches!(name, ".bashrc" | ".bash_profile" | ".profile") {
        return Some("shellscript");
    }
    match path.extension()?.to_str()? {
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("typescriptreact"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("javascriptreact"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "rs" => Some("rust"),
        "py" | "pyi" | "pyw" => Some("python"),
        "go" => Some("go"),
        "groovy" | "gvy" | "gy" | "gsh" | "gradle" => Some("groovy"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "scss" => Some("scss"),
        "less" => Some("less"),
        "vue" => Some("vue"),
        "svelte" => Some("svelte"),
        "json" => Some("json"),
        "jsonc" => Some("jsonc"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "xml" | "xsd" | "xsl" | "xslt" | "svg" | "pom" => Some("xml"),
        "properties" => Some("properties"),
        "env" => Some("dotenv"),
        "sql" => Some("sql"),
        "md" | "markdown" | "mdown" => Some("markdown"),
        "sh" | "bash" => Some("shellscript"),
        "mk" | "mak" => Some("makefile"),
        _ => None,
    }
}

pub fn language_family(language: &str) -> &str {
    match language {
        "typescriptreact" | "javascript" | "javascriptreact" => "typescript",
        "scss" | "less" => "css",
        "jsonc" => "json",
        other => other,
    }
}

pub(crate) struct Recipe {
    pub id: &'static str,
    pub name: &'static str,
    pub languages: &'static [&'static str],
    pub source: &'static str,
    pub license: &'static str,
    pub npm: &'static [&'static str],
    pub executable: &'static str,
    pub args: &'static [&'static str],
    pub limitations: &'static [&'static str],
}

pub(crate) const RECIPES: &[Recipe] = &[
    Recipe {
        id: "typescript",
        name: "TypeScript / JavaScript",
        languages: &[
            "typescript",
            "typescriptreact",
            "javascript",
            "javascriptreact",
        ],
        source: "https://github.com/typescript-language-server/typescript-language-server",
        license: "Apache-2.0 (server); Apache-2.0 (TypeScript)",
        npm: &["typescript-language-server", "typescript"],
        executable: "typescript-language-server",
        args: &["--stdio"],
        limitations: &[],
    },
    Recipe {
        id: "java",
        name: "Eclipse JDT LS",
        languages: &["java"],
        source: "https://github.com/eclipse-jdtls/eclipse.jdt.ls",
        license: "EPL-2.0 and included dependency notices",
        npm: &[],
        executable: "jdtls",
        args: &[
            "-data",
            "{workspace}",
            "-configuration",
            "{workspace}/configuration",
        ],
        limitations: &[
            "Maven/Gradle import requires separate project permission; JDT LS cannot guarantee Kotlin caller references.",
        ],
    },
    Recipe {
        id: "kotlin",
        name: "JetBrains Kotlin LSP",
        languages: &["kotlin"],
        source: "https://github.com/Kotlin/kotlin-lsp",
        license: "Partially proprietary; official first-use download only",
        npm: &[],
        executable: "kotlin-lsp",
        args: &[
            "--stdio",
            "--system-path",
            "{workspace}/system",
            "--data-sharing",
            "none",
        ],
        limitations: &[
            "Official alpha server, partially closed source. Installation does not grant Gradle/Maven import permission.",
        ],
    },
    Recipe {
        id: "rust",
        name: "rust-analyzer",
        languages: &["rust"],
        source: "https://github.com/rust-lang/rust-analyzer",
        license: "MIT OR Apache-2.0",
        npm: &[],
        executable: "rust-analyzer",
        args: &[],
        limitations: &[
            "Project Cargo import, build scripts and procedural macros require separate permission. Managed stable Rust compiler, standard library and Cargo are installed. Projects requiring another exact toolchain must use an explicit custom configuration.",
        ],
    },
    Recipe {
        id: "python",
        name: "Pyright",
        languages: &["python"],
        source: "https://github.com/microsoft/pyright",
        license: "MIT",
        npm: &["pyright"],
        executable: "pyright-langserver",
        args: &["--stdio"],
        limitations: &[
            "Static analysis works without Python; project interpreter and third-party packages improve import resolution. Pyright does not provide formatting.",
        ],
    },
    Recipe {
        id: "go",
        name: "gopls",
        languages: &["go"],
        source: "https://github.com/golang/tools/tree/master/gopls",
        license: "BSD-3-Clause",
        npm: &[],
        executable: "gopls",
        args: &[],
        limitations: &[
            "Installs managed Go. Go project loading may invoke go tooling and requires project import permission.",
        ],
    },
    Recipe {
        id: "groovy",
        name: "Groovy Language Server",
        languages: &["groovy"],
        source: "https://github.com/GroovyLanguageServer/groovy-language-server",
        license: "Apache-2.0",
        npm: &[],
        executable: "groovy-language-server",
        args: &[],
        limitations: &[
            "Groovy syntax and symbols cover .gradle files; Gradle DSL/classpath integration is limited. Managed install compiles a reviewed SHA-pinned upstream source commit using managed javac, with no Gradle scripts or annotation processors. Updates to this source pin require a new app catalog.",
        ],
    },
    Recipe {
        id: "web",
        name: "VS Code HTML / CSS / JSON servers",
        languages: &["html", "css", "scss", "less", "json", "jsonc"],
        source: "https://github.com/hrsh7th/vscode-langservers-extracted",
        license: "MIT",
        npm: &["vscode-langservers-extracted"],
        executable: "vscode-html-language-server",
        args: &["--stdio"],
        limitations: &[
            "Features vary per language; references and signature help are available only when the server advertises them.",
        ],
    },
    Recipe {
        id: "vue",
        name: "Vue Language Server",
        languages: &["vue"],
        source: "https://github.com/vuejs/language-tools",
        license: "MIT",
        npm: &["@vue/language-server", "typescript"],
        executable: "vue-language-server",
        args: &["--stdio"],
        limitations: &[
            "Vue 3 requires a TypeScript request bridge. Managed install uses the standalone 2.2.12 non-hybrid adapter with managed TypeScript; update remains on this compatible line until a bridge is implemented.",
        ],
    },
    Recipe {
        id: "svelte",
        name: "Svelte Language Server",
        languages: &["svelte"],
        source: "https://github.com/sveltejs/language-tools",
        license: "MIT",
        npm: &["svelte-language-server", "typescript"],
        executable: "svelteserver",
        args: &["--stdio"],
        limitations: &[
            "Loading project svelte.config.js executes project code and requires import permission.",
        ],
    },
    Recipe {
        id: "yaml",
        name: "YAML Language Server",
        languages: &["yaml"],
        source: "https://github.com/redhat-developer/yaml-language-server",
        license: "MIT",
        npm: &["yaml-language-server"],
        executable: "yaml-language-server",
        args: &["--stdio"],
        limitations: &[
            "Schema-dependent completion and validation; schema downloads disabled by default.",
        ],
    },
    Recipe {
        id: "toml",
        name: "Taplo",
        languages: &["toml"],
        source: "https://github.com/tamasfe/taplo",
        license: "MIT",
        npm: &[],
        executable: "taplo",
        args: &["lsp", "stdio"],
        limitations: &["Pinned official native distribution includes LSP; npm WASM CLI lacks LSP."],
    },
    Recipe {
        id: "xml",
        name: "Eclipse LemMinX",
        languages: &["xml"],
        source: "https://github.com/eclipse-lemminx/lemminx",
        license: "EPL-2.0 and included dependency notices",
        npm: &[],
        executable: "lemminx",
        args: &[],
        limitations: &["Uses managed Java. External schema/entity downloads disabled by default."],
    },
    Recipe {
        id: "sql",
        name: "SQLs",
        languages: &["sql"],
        source: "https://github.com/sqls-server/sqls",
        license: "MIT",
        npm: &[],
        executable: "sqls",
        args: &[],
        limitations: &[
            "Schema-aware features require explicit database configuration; no credentials/connections are provisioned. Pure-Go build supports MySQL/PostgreSQL; Oracle registration is omitted and SQLite requires a separately configured CGO-capable server.",
        ],
    },
    Recipe {
        id: "markdown",
        name: "Marksman",
        languages: &["markdown"],
        source: "https://github.com/artempyanykh/marksman",
        license: "MIT",
        npm: &[],
        executable: "marksman",
        args: &["server"],
        limitations: &[
            "Link and heading navigation; no general code completion, signature help or document formatter.",
        ],
    },
    Recipe {
        id: "shell",
        name: "Bash Language Server",
        languages: &["shellscript"],
        source: "https://github.com/bash-lsp/bash-language-server",
        license: "MIT",
        npm: &["bash-language-server"],
        executable: "bash-language-server",
        args: &["start"],
        limitations: &[
            "ShellCheck linting and shfmt formatting are optional external tools; unavailable until explicitly configured. No shell script execution.",
        ],
    },
    Recipe {
        id: "dockerfile",
        name: "Dockerfile Language Server",
        languages: &["dockerfile"],
        source: "https://github.com/rcjsuen/dockerfile-language-server",
        license: "MIT",
        npm: &["dockerfile-language-server-nodejs"],
        executable: "docker-langserver",
        args: &["--stdio"],
        limitations: &["Static Dockerfile language analysis; never invokes Docker."],
    },
    Recipe {
        id: "makefile",
        name: "make-ls",
        languages: &["makefile"],
        source: "https://github.com/owenrumney/make-ls",
        license: "MIT",
        npm: &[],
        executable: "make-ls",
        args: &[],
        limitations: &["Young server; statically parses Makefiles without running make."],
    },
];

pub fn language_catalog() -> Vec<LanguageSupport> {
    let mut result: Vec<_> = RECIPES
        .iter()
        .flat_map(|r| {
            r.languages.iter().map(|language| LanguageSupport {
                language: (*language).into(),
                name: r.name.into(),
                package: Some(r.id.into()),
                license: r.license.into(),
                source_url: r.source.into(),
                limitations: r.limitations.iter().map(|s| (*s).into()).collect(),
            })
        })
        .collect();
    result.push(LanguageSupport {
        language:"properties".into(),name:"Properties / Spring Boot configuration".into(),package:None,
        license:"Spring Tools extension: EPL-1.0 plus included dependency notices".into(),
        source_url:"https://github.com/spring-projects/spring-tools/blob/main/vscode-extensions/vscode-spring-boot/README.md".into(),
        limitations:vec!["Generic Properties: viewing and text search in this backend. Maintained free Spring Tools supports application*.properties completion, validation and hover, but requires Java project classpath metadata. Forkarak does not yet supply the STS addClasspathListener/javaType/javaLocation bridge or matching JDT extension bundles; a managed JVM alone is insufficient. No Spring semantic support is claimed until that bridge and explicit import flow are integrated.".into()],
    });
    result.push(LanguageSupport {
        language:"dotenv".into(),name:"Environment files".into(),package:None,license:String::new(),source_url:String::new(),
        limitations:vec!["Viewing and text search in this backend. No generic standalone dotenv adapter has been verified; framework-specific language servers do not establish general dotenv semantic support.".into()],
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expanded_paths_and_families() {
        for (path, lang) in [
            ("x.rs", "rust"),
            ("x.pyi", "python"),
            ("x.go", "go"),
            ("build.gradle", "groovy"),
            ("x.scss", "scss"),
            ("x.vue", "vue"),
            ("x.svelte", "svelte"),
            ("x.jsonc", "jsonc"),
            ("a.yml", "yaml"),
            ("Cargo.toml", "toml"),
            ("pom.xml", "xml"),
            ("x.properties", "properties"),
            (".env.local", "dotenv"),
            ("a.sql", "sql"),
            ("README.md", "markdown"),
            (".bashrc", "shellscript"),
            ("Dockerfile.dev", "dockerfile"),
            ("Makefile", "makefile"),
        ] {
            assert_eq!(language_for_path(Path::new(path)), Some(lang), "{path}");
        }
        assert_eq!(language_family("jsonc"), "json");
        assert_eq!(language_family("scss"), "css");
        assert!(
            language_catalog()
                .iter()
                .filter(|l| matches!(l.language.as_str(), "properties" | "dotenv"))
                .all(|l| l.package.is_none())
        );
    }
}
