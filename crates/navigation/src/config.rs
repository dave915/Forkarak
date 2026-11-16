use crate::ServerConfig;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// All waits happen on the caller's background thread. Construction never starts a server.
#[derive(Clone, Debug)]
pub struct NavigatorOptions {
    pub initialize_timeout: Duration,
    pub request_timeout: Duration,
    pub shutdown_timeout: Duration,
    /// None disables user configuration, useful for isolated callers and tests.
    pub config_file: Option<PathBuf>,
    /// Kotlin servers can run project import tools. Requires explicit opt-in.
    pub allow_project_imports: bool,
}

impl Default for NavigatorOptions {
    fn default() -> Self {
        Self {
            initialize_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(2),
            config_file: env::var_os("HOME")
                .map(|p| PathBuf::from(p).join(".config/pokarak/language-servers.json")),
            allow_project_imports: false,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    servers: Vec<ServerConfig>,
    #[serde(default, rename = "allowProjectImports")]
    allow_project_imports: bool,
}

pub(crate) fn merge_user_config(
    configs: &mut Vec<ServerConfig>,
    options: &mut NavigatorOptions,
) -> Result<()> {
    if let Some(path) = &options.config_file {
        let text = match fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Cannot read language-server config {}", path.display())
                });
            }
        };
        if let Some(text) = text {
            let user: ConfigFile = serde_json::from_str(&text)
                .with_context(|| format!("Invalid language-server config {}", path.display()))?;
            options.allow_project_imports |= user.allow_project_imports;
            for mut config in user.servers {
                config.language = normalize_language(&config.language).into();
                configs.retain(|old| normalize_language(&old.language) != config.language);
                configs.push(config);
            }
        }
    }
    for config in configs {
        config.language = normalize_language(&config.language).into();
        if !matches!(
            config.language.as_str(),
            "typescript" | "typescriptreact" | "javascript" | "javascriptreact" | "java" | "kotlin"
        ) {
            bail!("Unsupported language-server language {:?}", config.language);
        }
        if config.program.trim().is_empty() {
            bail!("Empty language-server program for {}", config.language);
        }
    }
    Ok(())
}

pub(crate) fn normalize_language(language: &str) -> &str {
    match language {
        "ts" | "js/ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" => "javascript",
        "jsx" => "javascriptreact",
        "kt" | "kts" => "kotlin",
        other => other,
    }
}

pub(crate) fn language(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("typescriptreact"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("javascriptreact"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        _ => None,
    }
}

pub(crate) fn family(language: &str) -> &str {
    match language {
        "typescriptreact" | "javascript" | "javascriptreact" => "typescript",
        other => other,
    }
}

/// Discover an executable on the inherited PATH. Never searches project node_modules,
/// invokes a shell, runs npx, installs software, or starts the discovered program.
pub fn discover_program(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() {
        return executable(path).then(|| path.to_path_buf());
    }
    if path.components().count() != 1 {
        return None;
    }
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        // Relative PATH entries could execute code in the inspected repository.
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|path| executable(path))
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn default_configs() -> Vec<ServerConfig> {
    // A family entry also covers JS/JSX/TSX; exact language overrides take priority.
    [
        ("typescript", "typescript-language-server", vec!["--stdio"]),
        (
            "java",
            "jdtls",
            vec![
                "-data",
                "{workspace}",
                "-configuration",
                "{workspace}/configuration",
            ],
        ),
        ("kotlin", "kotlin-language-server", vec![]),
    ]
    .into_iter()
    .map(|(language, program, args)| ServerConfig {
        language: language.into(),
        program: discover_program(program)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| program.into()),
        args: args.into_iter().map(str::to_owned).collect(),
    })
    .collect()
}
