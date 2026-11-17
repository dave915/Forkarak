pub mod ctl;

use std::{ffi::OsString, path::PathBuf};

pub struct Options {
    pub projects: Vec<PathBuf>,
    pub session_dir: Option<PathBuf>,
    pub width: f32,
    pub height: f32,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Options, String> {
    let mut args = args.into_iter();
    let mut options = Options {
        projects: vec![],
        session_dir: None,
        width: 1440.,
        height: 900.,
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
        } else if !paths_only && arg == "--window-size" {
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
            return Err(format!("Unknown option: {}", arg.to_string_lossy()));
        } else {
            options.projects.push(PathBuf::from(arg));
        }
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;
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
