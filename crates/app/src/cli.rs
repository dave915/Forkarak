pub mod ctl;
pub mod launch;
pub use launch::{launch, resolve};

use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::PathBuf};

pub const HELP: &str = "Forkarak — native macOS code editor and Git worktrees

Usage: forkarak [OPTIONS] [--] [FILE|DIRECTORY ...]
       fkk [OPTIONS] [--] [FILE|DIRECTORY ...]
       forkarak -g FILE:LINE[:COLUMN]
       forkarak ctl help    (headless JSON API / agent instructions)
       forkarak install-cli [--bin-dir DIRECTORY] (local bundle; no download)

  -g, --goto FILE:LINE[:COLUMN]  Go to one-based line/Unicode character column
  -r, --reuse-window            Reuse a unique matching window (or sole window)
  -n, --new-window              Always launch a new window
      --background, --no-launch Reuse only; never activate or launch an app
      --session SOCKET         Explicit reuse target from ctl sessions
      --session-dir DIR        Isolated application state (also scopes reuse)
      --window-size WIDTHxHEIGHT  New window size; minimum 940x580
  -h, --help                   Show this help
  -v, --version                Show version

Existing files/directories in Git worktrees only; directories and . open projects.
Plain paths preserve colons. With --goto, an existing whole filename wins; otherwise
the numeric suffix specifies the location. Use -- before literal reserved names
(ctl, update, help, install-cli) or names beginning with -. Multiple paths/goto flags are allowed.
Default: reuse a unique window containing all requested worktrees; otherwise new.
Reused windows are shown unless --background/--no-launch is given. Ambiguous reuse fails; select --session or --new-window.
Loose files, new file creation, --wait and --diff are not supported.
Installed commands also support: forkarak update [--version vX.Y.Z] (quit app first).";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub line: u32,
    pub column: u32,
}
impl Location {
    /// Human coordinates count Unicode characters, not bytes or UTF-16 units.
    pub fn utf16(self, text: &str) -> Result<(u32, u32), String> {
        if self.line == 0 || self.column == 0 {
            return Err("Line and column must be positive one-based coordinates".into());
        }
        let line = text
            .split('\n')
            .nth((self.line - 1) as usize)
            .ok_or_else(|| format!("Line {} is outside the displayed file", self.line))?
            .trim_end_matches('\r');
        if self.column as usize > line.chars().count() + 1 {
            return Err(format!(
                "Column {} is outside line {}",
                self.column, self.line
            ));
        }
        Ok((
            self.line - 1,
            line.chars()
                .take((self.column - 1) as usize)
                .map(char::len_utf16)
                .sum::<usize>() as u32,
        ))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenTarget {
    pub repo: PathBuf,
    pub path: Option<PathBuf>,
    pub location: Option<Location>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowMode {
    #[default]
    Auto,
    Reuse,
    New,
}

pub struct Options {
    pub projects: Vec<PathBuf>,
    pub session_dir: Option<PathBuf>,
    pub width: f32,
    pub height: f32,
    pub locations: Vec<Option<Location>>,
    pub window_mode: WindowMode,
    pub session: Option<PathBuf>,
    pub window_size_given: bool,
    pub background: bool,
}

fn goto(value: OsString) -> Result<(PathBuf, Option<Location>), String> {
    let path = PathBuf::from(&value);
    // Never reinterpret a real colon-containing filename as a different file.
    if path.exists() {
        if !path.is_file() {
            return Err("--goto requires a file, not a directory".into());
        }
        return Ok((path, None));
    }
    let text = value
        .to_str()
        .ok_or("--goto requires a UTF-8 path/location")?;
    let (prefix, last) = text
        .rsplit_once(':')
        .ok_or("Use --goto FILE:LINE[:COLUMN]")?;
    let number = |s: &str| {
        s.parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| "Line and column must be positive one-based integers".to_string())
    };
    let last = number(last)?;
    // Prefer an existing prefix for FILE:LINE, even if that filename ends in :N.
    let (path, line, column) = if PathBuf::from(prefix).is_file() {
        (prefix, last, 1)
    } else if let Some((file, line)) = prefix.rsplit_once(':') {
        if line.bytes().all(|c| c.is_ascii_digit()) && !line.is_empty() {
            (file, number(line)?, last)
        } else {
            (prefix, last, 1)
        }
    } else {
        (prefix, last, 1)
    };
    Ok((PathBuf::from(path), Some(Location { line, column })))
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Options, String> {
    let mut args = args.into_iter();
    let mut options = Options {
        projects: vec![],
        session_dir: None,
        width: 1440.,
        height: 900.,
        locations: vec![],
        window_mode: WindowMode::Auto,
        session: None,
        window_size_given: false,
        background: false,
    };
    let mut paths_only = false;
    while let Some(arg) = args.next() {
        if !paths_only && arg == "--" {
            paths_only = true;
            continue;
        }
        if !paths_only && arg == "--session-dir" {
            options.session_dir = Some(PathBuf::from(
                args.next().ok_or("--session-dir requires a directory")?,
            ));
        } else if !paths_only && arg == "--session" {
            options.session = Some(PathBuf::from(
                args.next()
                    .ok_or("--session requires a socket from ctl sessions")?,
            ));
        } else if !paths_only && (arg == "-g" || arg == "--goto") {
            let (path, location) = goto(args.next().ok_or("--goto requires FILE:LINE[:COLUMN]")?)?;
            options.projects.push(path);
            options.locations.push(location);
        } else if !paths_only
            && ["-r", "--reuse-window", "-n", "--new-window"]
                .iter()
                .any(|s| arg == *s)
        {
            let mode = if arg == "-r" || arg == "--reuse-window" {
                WindowMode::Reuse
            } else {
                WindowMode::New
            };
            if options.window_mode != WindowMode::Auto && options.window_mode != mode {
                return Err("--reuse-window and --new-window are mutually exclusive".into());
            }
            options.window_mode = mode;
        } else if !paths_only && (arg == "--background" || arg == "--no-launch") {
            options.background = true;
        } else if !paths_only && arg == "--window-size" {
            options.window_size_given = true;
            let size = args.next().ok_or("--window-size requires WIDTHxHEIGHT")?;
            let size = size.to_str().ok_or("Invalid window size")?;
            let (w, h) = size
                .split_once('x')
                .ok_or("Use WIDTHxHEIGHT, e.g. 1100x700")?;
            options.width = w.parse().map_err(|_| "Invalid width")?;
            options.height = h.parse().map_err(|_| "Invalid height")?;
            if !options.width.is_finite()
                || !options.height.is_finite()
                || options.width < 940.
                || options.height < 580.
            {
                return Err("Minimum window size is 940x580".into());
            }
        } else if !paths_only && arg.to_string_lossy().starts_with('-') {
            return Err(format!(
                "Unknown option: {}. Run forkarak --help; use -- before literal filenames",
                arg.to_string_lossy()
            ));
        } else {
            options.projects.push(PathBuf::from(arg));
            options.locations.push(None);
        }
    }
    if options.background {
        if options.window_mode == WindowMode::New || options.window_size_given {
            return Err("--background/--no-launch requires an existing window; cannot combine with --new-window or --window-size".into());
        }
        options.window_mode = WindowMode::Reuse;
    }
    if options.session.is_some() {
        if options.window_mode == WindowMode::New {
            return Err("--session cannot be combined with --new-window".into());
        }
        options.window_mode = WindowMode::Reuse;
    }
    if options.window_size_given && options.window_mode == WindowMode::Reuse {
        return Err("--window-size applies to new windows; use --new-window".into());
    }
    Ok(options)
}

/// Help/version are options only before --, and never consume a flag's value.
pub fn informational(args: &[OsString]) -> Option<bool> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg == "--help" || arg == "-h" {
            return Some(false);
        }
        if arg == "--version" || arg == "-v" {
            return Some(true);
        }
        if [
            "-g",
            "--goto",
            "--session",
            "--session-dir",
            "--window-size",
        ]
        .iter()
        .any(|s| arg == *s)
        {
            args.next();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_one_based_coordinates_reject_outside_and_surrogate_splits() {
        let text = "a😀한글\nsecond\n";
        assert_eq!(Location { line: 1, column: 3 }.utf16(text).unwrap(), (0, 3));
        assert_eq!(Location { line: 1, column: 5 }.utf16(text).unwrap(), (0, 5));
        assert_eq!(Location { line: 3, column: 1 }.utf16(text).unwrap(), (2, 0));
        for (line, column) in [(0, 1), (1, 0), (4, 1), (1, 6), (2, 8)] {
            assert!(Location { line, column }.utf16(text).is_err());
        }
    }
    #[test]
    fn goto_preserves_colon_filenames_and_parses_only_explicit_locations() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("name:12");
        std::fs::write(&file, "fixture").unwrap();
        let value = format!("{}:3", file.display());
        let options = parse(["-g".into(), value.into()]).unwrap();
        assert_eq!(options.projects.as_slice(), std::slice::from_ref(&file));
        assert_eq!(options.locations, [Some(Location { line: 3, column: 1 })]);
        assert_eq!(
            parse(["--goto".into(), file.clone().into()])
                .unwrap()
                .locations,
            [None]
        );
        let plain = format!("{}:3:4", file.display());
        let opts = parse([OsString::from(&plain)]).unwrap();
        assert_eq!(opts.projects, [PathBuf::from(&plain)]);
        let opts = parse(["-g".into(), plain.into()]).unwrap();
        assert_eq!(opts.projects, [file]);
        assert_eq!(opts.locations, [Some(Location { line: 3, column: 4 })]);
        for bad in [
            "file:0",
            "file:1:0",
            "file:1:bad",
            "file:4294967296",
            "file",
        ] {
            assert!(parse(["-g".into(), bad.into()]).is_err(), "{bad}");
        }
    }
    #[test]
    fn reuse_new_background_and_unsupported_flags_have_honest_semantics() {
        for args in [
            vec!["--reuse-window", "--new-window"],
            vec!["--background", "--new-window"],
            vec!["--no-launch", "--window-size", "1000x700"],
            vec!["--session", "socket", "--new-window"],
            vec!["--reuse-window", "--window-size", "1000x700"],
            vec!["--wait"],
            vec!["--diff"],
            vec!["--goto"],
        ] {
            assert!(parse(args.into_iter().map(OsString::from)).is_err());
        }
        assert_eq!(
            parse(["--background".into()]).unwrap().window_mode,
            WindowMode::Reuse
        );
        assert_eq!(informational(&["--".into(), "--help".into()]), None);
        assert_eq!(informational(&["-g".into(), "--help".into()]), None);
        assert_eq!(informational(&["-h".into()]), Some(false));
        assert_eq!(informational(&["--version".into()]), Some(true));
    }
    #[test]
    fn accepts_multiple_projects_and_window_size() {
        let args =
            ["/tmp/one project", "/tmp/한글", "--window-size", "940x580"].map(OsString::from);
        let options = parse(args).unwrap();
        assert_eq!(
            options.projects,
            vec![
                PathBuf::from("/tmp/one project"),
                PathBuf::from("/tmp/한글")
            ]
        );
        assert_eq!((options.width, options.height), (940., 580.));
    }
    #[test]
    fn rejects_invalid_sizes_and_accepts_literal_paths() {
        for value in ["NaNx800", "-1x800", "939x580", "1000", "1000xinf"] {
            assert!(parse([OsString::from("--window-size"), value.into()]).is_err());
        }
        assert_eq!(
            parse([OsString::from("--"), "-project".into()])
                .unwrap()
                .projects,
            vec![PathBuf::from("-project")]
        );
    }
}

#[cfg(test)]
mod ctl_tests;
