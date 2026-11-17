//! Editor-style launch preparation and private session routing.
use super::{OpenTarget, Options, WindowMode};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

pub fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    // Resolve existing paths before removing ..: symlink/.. follows the actual
    // filesystem, rather than a lexical guess at a different worktree.
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => (),
            std::path::Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other),
        }
    }
    for ancestor in clean.ancestors() {
        if let Ok(canonical) = ancestor.canonicalize() {
            return Ok(canonical.join(clean.strip_prefix(ancestor)?));
        }
    }
    Ok(clean)
}

pub fn validate_target(target: &OpenTarget) -> Result<()> {
    target
        .repo
        .to_str()
        .context("CLI opening requires UTF-8 worktree paths")?;
    ensure!(
        target.repo.is_absolute() && pokarak_core::repository_root(&target.repo)? == target.repo,
        "Open target must identify a canonical Git worktree"
    );
    if let Some(path) = &target.path {
        let relative = super::ctl::relative(&target.repo, path)?;
        ensure!(
            &relative == path,
            "Open target file must be worktree relative"
        );
        pokarak_core::read_file(&target.repo, path)?;
    } else {
        ensure!(
            target.location.is_none(),
            "--goto requires a file, not a directory"
        );
    }
    if let Some(location) = target.location {
        ensure!(
            location.line > 0 && location.column > 0,
            "Line and column must be positive"
        );
    }
    Ok(())
}

pub fn resolve(options: &Options) -> Result<Vec<OpenTarget>> {
    options.projects.iter().zip(&options.locations).map(|(path, location)| {
        let absolute = path.canonicalize().with_context(|| format!("{} does not exist or is inaccessible; CLI opens existing files/directories only", path.display()))?;
        let repo = pokarak_core::repository_root(&absolute).with_context(|| format!(
            "{}: CLI file/project opening currently requires a Git worktree. Loose files outside Git are not supported; no repository was created", path.display()))?;
        let file = if absolute.is_file() { Some(super::ctl::relative(&repo, &absolute)?) }
            else { ensure!(absolute.is_dir(), "Expected a regular file or directory"); None };
        let target = OpenTarget { repo, path: file, location: *location };
        validate_target(&target)?;
        Ok(target)
    }).collect()
}

fn matches(status: &Value, target: &OpenTarget) -> bool {
    let repo = serde_json::json!(target.repo);
    status.get("repo") == Some(&repo)
        || ["projects", "project_worktrees"].iter().any(|key| {
            status
                .get(*key)
                .and_then(Value::as_array)
                .is_some_and(|paths| paths.contains(&repo))
        })
        || status
            .get("worktrees")
            .and_then(Value::as_array)
            .is_some_and(|trees| {
                trees
                    .iter()
                    .any(|tree| tree.get("path") == Some(&serde_json::json!(target.repo)))
            })
}

/// Discovery is read-only. Activation is explicit in the final request; automation opts out.
pub fn reuse(options: &Options, targets: &[OpenTarget]) -> Result<bool> {
    if options.window_mode == WindowMode::New || options.window_size_given {
        return Ok(false);
    }
    if targets.is_empty() && options.session.is_none() && !options.background {
        return Ok(false);
    }
    let scope = options.session_dir.as_deref().map(absolute).transpose()?;
    let sockets: Vec<PathBuf> = if let Some(socket) = &options.session {
        vec![socket.clone()]
    } else {
        serde_json::from_value(crate::control::sessions()?["sessions"].clone())?
    };
    let mut live = Vec::new();
    for socket in sockets {
        match crate::control::call(&socket, &crate::control::Request::status()) {
            Ok(status) => {
                if scope.as_ref().is_some_and(|scope| {
                    status.get("session_dir") != Some(&serde_json::json!(scope))
                }) {
                    continue;
                }
                live.push((socket, status));
            }
            Err(error) if options.session.is_some() => return Err(error),
            Err(_) => (), // Removed/stale sockets do not select a different window.
        }
    }
    let matching: Vec<_> = live
        .iter()
        .filter(|(_, status)| targets.iter().all(|target| matches(status, target)))
        .collect();
    let chosen = if options.session.is_some() {
        ensure!(
            live.len() == 1,
            "Requested session does not match --session-dir"
        );
        live.first()
    } else if matching.len() == 1 {
        Some(matching[0])
    } else if matching.len() > 1 {
        anyhow::bail!(
            "Multiple windows contain these worktrees; use --session SOCKET or --new-window"
        );
    } else if options.window_mode == WindowMode::Reuse {
        ensure!(
            live.len() <= 1,
            "Multiple windows are open; use --session SOCKET from ctl sessions or --new-window"
        );
        live.first()
    } else {
        None
    };
    let Some((socket, _)) = chosen else {
        ensure!(
            !options.background,
            "No matching running window; --background/--no-launch never starts an app"
        );
        return Ok(false);
    };
    for (index, target) in targets.iter().enumerate() {
        let mut request = crate::control::Request::status();
        request.action = "open-target".into();
        request.target = Some(target.clone());
        request.activate = !options.background && index + 1 == targets.len();
        crate::control::call(socket, &request).with_context(|| {
            format!(
                "Could not open {} in the chosen window (earlier targets may already be open)",
                target.repo.display()
            )
        })?;
    }
    Ok(true)
}

/// Invoked by both installed aliases. All LaunchServices arguments are argv.
pub fn launch(app: &Path, args: &[OsString]) -> Result<()> {
    if let Some(version) = super::informational(args) {
        println!(
            "{}",
            if version {
                env!("CARGO_PKG_VERSION")
            } else {
                super::HELP
            }
        );
        return Ok(());
    }
    let options = super::parse(args.iter().cloned()).map_err(anyhow::Error::msg)?;
    let targets = resolve(&options)?;
    if reuse(&options, &targets)? {
        return Ok(());
    }
    for target in &targets {
        if let (Some(path), Some(location)) = (&target.path, target.location) {
            location
                .utf16(&pokarak_core::read_file(&target.repo, path)?)
                .map_err(anyhow::Error::msg)?;
        }
    }
    ensure!(
        app.is_absolute() && app.is_dir(),
        "Installed application bundle is unavailable"
    );
    let mut command = Command::new("/usr/bin/open");
    if !targets.is_empty()
        || options.window_mode == WindowMode::New
        || options.session_dir.is_some()
        || options.window_size_given
    {
        command.arg("-n").arg(app).arg("--args");
        command
            .arg("--cli-targets")
            .arg(serde_json::to_string(&targets)?);
        if let Some(dir) = &options.session_dir {
            command.arg("--session-dir").arg(absolute(dir)?);
        }
        if options.window_size_given {
            command
                .arg("--window-size")
                .arg(format!("{}x{}", options.width, options.height));
        }
    } else {
        command.arg(app);
    }
    ensure!(
        command
            .status()
            .context("Could not start Forkarak via LaunchServices")?
            .success(),
        "LaunchServices could not open Forkarak"
    );
    Ok(())
}

fn install_cli_command(executable: &Path, args: &[OsString]) -> Result<Command> {
    let executable = executable.canonicalize()?;
    let macos = executable
        .parent()
        .context("Missing bundle executable directory")?;
    let contents = macos.parent().context("Missing bundle Contents")?;
    let bundle = contents.parent().context("Missing application bundle")?;
    ensure!(
        macos.file_name().is_some_and(|name| name == "MacOS")
            && contents.file_name().is_some_and(|name| name == "Contents")
            && bundle.extension().is_some_and(|name| name == "app"),
        "Run install-cli from a packaged Forkarak.app/Contents/MacOS/forkarak; it installs that local bundle without downloading a release"
    );
    let installer = contents.join("Resources/install.sh");
    ensure!(
        installer.is_file(),
        "Local bundle is missing Resources/install.sh; rebuild the package"
    );
    let mut command = Command::new("/bin/bash");
    command
        .arg(installer)
        .arg("--install-cli")
        .arg(bundle)
        .args(args);
    Ok(command)
}

pub fn install_cli(args: &[OsString]) -> Result<i32> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Usage: Forkarak.app/Contents/MacOS/forkarak install-cli [--bin-dir DIRECTORY]\nInstalls forkarak and fkk from this signed local bundle, without downloads.\nDefault: ~/.local/bin. Existing unrelated commands and symlinks are preserved.\nThe app bundle stays in its current location; keep it there while using these commands."
        );
        return Ok(0);
    }
    let mut arguments = args.iter();
    while let Some(arg) = arguments.next() {
        ensure!(
            arg == "--bin-dir",
            "Unknown install-cli option: {} (use install-cli --help)",
            arg.to_string_lossy()
        );
        arguments.next().context("--bin-dir requires a directory")?;
    }
    Ok(install_cli_command(&std::env::current_exe()?, args)?
        .status()?
        .code()
        .unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Location, parse};
    use std::{fs, sync::mpsc, thread, time::Duration};
    fn options(args: &[&str]) -> Options {
        parse(args.iter().map(OsString::from)).unwrap()
    }
    fn git(root: &Path, args: &[&str]) {
        let result = Command::new("git")
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        fs::write(dir.path().join("한글 space.rs"), "a😀한글\nsecond\n").unwrap();
        git(dir.path(), &["add", "--", "한글 space.rs"]);
        git(dir.path(), &["commit", "-m", "fixture", "--"]);
        dir
    }
    #[test]
    fn resolves_exact_files_directories_and_linked_worktrees_without_mutation() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("-literal"), "literal").unwrap();
        fs::write(root.join("ctl"), "reserved").unwrap();
        let args: Vec<_> = [
            root.join("한글 space.rs"),
            root.join("nested"),
            root.join("-literal"),
            root.join("ctl"),
        ]
        .into_iter()
        .map(PathBuf::into_os_string)
        .collect();
        let targets = resolve(&parse(args).unwrap()).unwrap();
        assert_eq!(targets.len(), 4);
        assert!(targets.iter().all(|target| target.repo == root));
        assert_eq!(targets[0].path, Some("한글 space.rs".into()));
        assert_eq!(targets[1].path, None);
        let parent = tempfile::tempdir().unwrap();
        let linked = parent.path().join("linked 한글 space");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "linked",
                linked.to_str().unwrap(),
                "HEAD",
                "--",
            ],
        );
        assert!(linked.join(".git").is_file());
        let opts = options(&[linked.join("한글 space.rs").to_str().unwrap()]);
        let target = resolve(&opts).unwrap().pop().unwrap();
        assert_eq!(target.repo, linked.canonicalize().unwrap());
        assert_eq!(target.path, Some("한글 space.rs".into()));
        assert_eq!(
            fs::read_to_string(root.join("한글 space.rs")).unwrap(),
            "a😀한글\nsecond\n"
        );
        assert!(!root.join(".git/index.lock").exists());
    }
    #[test]
    fn rejects_loose_missing_binary_metadata_and_outside_targets() {
        let loose = tempfile::tempdir().unwrap();
        let loose_file = loose.path().join("loose.txt");
        fs::write(&loose_file, "loose").unwrap();
        let error = resolve(&options(&[loose_file.to_str().unwrap()]))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Loose files outside Git are not supported"),
            "{error}"
        );
        assert!(!loose.path().join(".git").exists());
        let dir = fixture();
        fs::write(dir.path().join("binary"), [0, 0xff]).unwrap();
        for file in ["missing", "binary", ".git/config"] {
            assert!(
                resolve(&options(&[dir.path().join(file).to_str().unwrap()])).is_err(),
                "{file}"
            );
        }
        assert!(
            validate_target(&OpenTarget {
                repo: dir.path().canonicalize().unwrap(),
                path: Some("../loose.txt".into()),
                location: None
            })
            .is_err()
        );
    }
    #[test]
    fn private_session_transport_preserves_target_and_activation_intent() {
        let dir = fixture();
        for (flags, activate) in [
            (vec![], true),
            (vec!["--background"], false),
            (vec!["--no-launch"], false),
        ] {
            let server = crate::control::Server::start().unwrap();
            let mut args = vec!["--session", server.socket.to_str().unwrap()];
            args.extend(flags);
            let opts = options(&args);
            let target = OpenTarget {
                repo: dir.path().canonicalize().unwrap(),
                path: Some("한글 space.rs".into()),
                location: Some(Location { line: 1, column: 3 }),
            };
            let (send, receive) = mpsc::channel();
            let worker = thread::spawn(move || {
                let status = server
                    .receiver
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
                assert_eq!(status.request.action, "status");
                status
                    .reply
                    .send(Ok(serde_json::json!({"repo":target.repo})))
                    .unwrap();
                let open = server
                    .receiver
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
                assert_eq!(open.request.action, "open-target");
                assert_eq!(open.request.activate, activate);
                let requested = open.request.target.as_ref().unwrap();
                assert_eq!(requested.repo, target.repo);
                assert_eq!(requested.path, target.path);
                assert_eq!(requested.location, target.location);
                open.reply
                    .send(Ok(serde_json::json!({"opened":true})))
                    .unwrap();
                send.send(()).unwrap();
            });
            assert!(
                reuse(
                    &opts,
                    &[OpenTarget {
                        repo: dir.path().canonicalize().unwrap(),
                        path: Some("한글 space.rs".into()),
                        location: Some(Location { line: 1, column: 3 })
                    }]
                )
                .unwrap()
            );
            receive.recv_timeout(Duration::from_secs(3)).unwrap();
            worker.join().unwrap();
        }
    }
}
