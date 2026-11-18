//! Headless control dispatch. All inspection stays in pokarak_core.
use anyhow::{Context, Result, ensure};
use pokarak_core::{
    CompareMode,
    operations::{
        self, GitOperations, OperationOutput,
        commit_actions::{CommitMode, StageScope},
        integration::IntegrationKind,
    },
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const USAGE: &str = "forkarak ctl <command> [options]\nRead: status/worktrees/branches/files [--changed [--name-only]]/search --query TEXT [--limit N]/diff --path FILE/read --path FILE\nAll repository commands require --repo PATH (absolute or relative selected worktree). status/files/diff accept --base BRANCH. files --changed --name-only returns paths under files and skips line statistics.\nWrite: write --path FILE --expected-file BEFORE --content-file AFTER | stage/unstage --path FILE [--path FILE ...] | commit --message TEXT | merge --branch BRANCH\nAll writes require --repo PATH --yes. write replaces an existing UTF-8 file only if exact expected bytes still match.\nGUI: sessions | gui status --session SOCKET | gui open-project/select-worktree/refresh --session SOCKET --repo PATH | gui open-file --session SOCKET --repo PATH --path FILE [--line N] | gui view --session SOCKET --repo PATH --mode graph|code\nAgent guide: instructions [--agent claude|codex]\nEvery ctl command returns one JSON object on stdout; ok=false means a nonzero exit. File paths are relative to the worktree, not the shell cwd.";

pub fn instructions(agent: &str) -> String {
    format!(
        "# Forkarak instructions for {agent}\n\nUse `forkarak ctl` to inspect the explicitly selected Git worktree. First run `forkarak ctl status --repo /absolute/worktree`. Read result.repo to verify the canonical target. Never assume the GUI's selected worktree. All responses are JSON: check ok before reading result; errors exit nonzero.\n\n```sh\nforkarak ctl worktrees --repo /absolute/worktree\nforkarak ctl branches --repo /absolute/worktree\nforkarak ctl files --repo /absolute/worktree --changed\nforkarak ctl search --repo /absolute/worktree --query 'literal text' --limit 50\nforkarak ctl diff --repo /absolute/worktree --path src/main.rs\nforkarak ctl read --repo /absolute/worktree --path src/main.rs\n```\n\nSearch is literal text, never semantic references; locations are zero-based UTF-16. `--base main` on status/files/diff includes branch changes against merge-base. Read, diff and search never start language servers, builds or Git writes. Paths with spaces must be quoted. Prefix a repository literally named ctl with `./` for GUI launch, or use `forkarak -- ctl`.\n\nOnly perform a write when the user requests it. Every write needs `--yes`. For editing, decode result.text from read into a temporary UTF-8 expected file outside the worktree; put the complete replacement in another file. Do not redirect read's JSON envelope into the expected file.\n\n```sh\nforkarak ctl write --repo /absolute/worktree --path src/main.rs --expected-file /tmp/before.txt --content-file /tmp/after.txt --yes\nforkarak ctl stage --repo /absolute/worktree --path src/main.rs --yes\nforkarak ctl unstage --repo /absolute/worktree --path src/main.rs --yes\nforkarak ctl commit --repo /absolute/worktree --message 'Describe the change' --yes\nforkarak ctl merge --repo /absolute/worktree --branch feature/example --yes\n```\n\nSave rejects stale bytes, traversal, symlinks, Git metadata, binary and oversized files. It does not stage. Stage/unstage accept only named files; commit uses only the current index. Merge requires a clean worktree and guards source/destination refs. Hooks/filters can run only on explicit Git writes. A failed Git write can leave conflicts or partial state: inspect status and stop for user direction; never silently retry, force, discard, reset, stash or checkout.\n\nFor a running GUI, call `forkarak ctl sessions`, choose the intended socket, then `forkarak ctl gui status --session SOCKET`. Open a project with `gui open-project --session SOCKET --repo PATH`; wait until status shows the intended repo and loading=false before `gui open-file`, `gui view` or `gui refresh`. GUI success acknowledges navigation scheduling; status reports loading/error. GUI controls never authorize writes, trust, builds or language analysis.\n"
    )
}

#[derive(Default)]
pub(super) struct Flags {
    values: HashMap<String, Vec<OsString>>,
    switches: HashSet<String>,
}
impl Flags {
    fn parse(
        args: &[OsString],
        values: &[&str],
        switches: &[&str],
        repeated: &[&str],
    ) -> Result<Self> {
        let mut flags = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let flag = arg.to_str().context("Options must be UTF-8")?;
            if switches.contains(&flag) {
                ensure!(
                    flags.switches.insert(flag.into()),
                    "Duplicate option: {flag}"
                );
            } else {
                ensure!(values.contains(&flag), "Unknown option: {flag}");
                let value = args
                    .next()
                    .with_context(|| format!("{flag} requires a value"))?;
                let list = flags.values.entry(flag.into()).or_default();
                ensure!(
                    list.is_empty() || repeated.contains(&flag),
                    "Duplicate option: {flag}"
                );
                list.push(value.clone());
            }
        }
        Ok(flags)
    }
    pub(super) fn path(&self, key: &str) -> Result<PathBuf> {
        Ok(PathBuf::from(
            self.values
                .get(key)
                .and_then(|v| v.first())
                .with_context(|| format!("{key} is required"))?,
        ))
    }
    pub(super) fn text(&self, key: &str) -> Result<&str> {
        self.values
            .get(key)
            .and_then(|v| v.first())
            .with_context(|| format!("{key} is required"))?
            .to_str()
            .context("Text arguments must be UTF-8")
    }
    pub(super) fn optional(&self, key: &str) -> Result<Option<&str>> {
        self.values.get(key).map(|_| self.text(key)).transpose()
    }
    pub(super) fn number(&self, key: &str, default: usize, max: usize) -> Result<usize> {
        let n = self
            .optional(key)?
            .map(str::parse::<usize>)
            .transpose()
            .with_context(|| format!("{key} requires a positive integer"))?
            .unwrap_or(default);
        ensure!(n > 0 && n <= max, "{key} must be between 1 and {max}");
        Ok(n)
    }
}

pub(crate) fn relative(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.strip_prefix(root)
            .context("File is outside selected worktree")?
    } else {
        path
    };
    let path: PathBuf = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    ensure!(
        !path.as_os_str().is_empty()
            && path.components().all(
                |c| matches!(c, Component::Normal(name) if !name.eq_ignore_ascii_case(".git"))
            ),
        "Select a file without traversal or .git components"
    );
    path.to_str()
        .context("JSON control requires a UTF-8 file path")?;
    Ok(path)
}
fn input_file(path: PathBuf) -> Result<String> {
    let file =
        std::fs::File::open(&path).with_context(|| format!("Cannot read {}", path.display()))?;
    ensure!(
        file.metadata()?.is_file(),
        "Expected/content input must be a regular file"
    );
    let mut bytes = Vec::new();
    file.take(pokarak_core::MAX_TEXT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= pokarak_core::MAX_TEXT_BYTES,
        "Input exceeds the 2 MiB limit"
    );
    String::from_utf8(bytes).context("Expected/content input must be UTF-8")
}
fn output(value: OperationOutput) -> Value {
    json!({"repo":value.root,"stdout":value.stdout,"stderr":value.stderr,"truncated":value.truncated})
}

fn run(args: &[OsString]) -> Result<Value> {
    let command = args.first().and_then(|s| s.to_str()).unwrap_or("help");
    let rest = args.get(1..).unwrap_or_default();
    if ["help", "--help", "-h"].contains(&command) {
        ensure!(rest.is_empty(), "Unexpected help arguments");
        return Ok(json!({"usage":USAGE}));
    }
    if command == "instructions" {
        let flags = Flags::parse(rest, &["--agent"], &[], &[])?;
        let agent = flags.optional("--agent")?.unwrap_or("Claude and Codex");
        ensure!(
            ["claude", "codex", "Claude and Codex"].contains(&agent),
            "--agent must be claude or codex"
        );
        return Ok(json!({"instructions":instructions(agent),"usage":USAGE}));
    }
    if command == "sessions" {
        ensure!(rest.is_empty(), "sessions takes no options");
        return crate::control::sessions();
    }
    if command == "gui" {
        return gui(rest);
    }
    let (values, switches): (&[&str], &[&str]) = match command {
        "status" => (&["--repo", "--base"], &[]),
        "diff" => (&["--repo", "--base", "--path"], &[]),
        "files" => (&["--repo", "--base"], &["--changed", "--name-only"]),
        "worktrees" | "branches" => (&["--repo"], &[]),
        "search" => (&["--repo", "--query", "--limit"], &[]),
        "read" => (&["--repo", "--path"], &[]),
        "write" => (
            &["--repo", "--path", "--expected-file", "--content-file"],
            &["--yes"],
        ),
        "stage" | "unstage" => (&["--repo", "--path"], &["--yes"]),
        "commit" => (&["--repo", "--message"], &["--yes"]),
        "merge" => (&["--repo", "--branch"], &["--yes"]),
        _ => anyhow::bail!("Unknown ctl command: {command}. Use forkarak ctl help"),
    };
    let flags = Flags::parse(
        rest,
        values,
        switches,
        if ["stage", "unstage"].contains(&command) {
            &["--path"]
        } else {
            &[]
        },
    )?;
    if switches.contains(&"--yes") {
        ensure!(
            flags.switches.contains("--yes"),
            "Explicit --yes is required for {command}"
        );
    }
    let repo = flags.path("--repo")?;
    ensure!(
        repo.is_dir(),
        "--repo must name an existing worktree directory"
    );
    let root = pokarak_core::repository_root(&repo)?;
    root.to_str()
        .context("JSON control requires a UTF-8 worktree path")?;
    let compare = flags
        .optional("--base")?
        .map(|s| CompareMode::Branch(s.into()))
        .unwrap_or_default();
    match command {
        "worktrees" => Ok(
            json!({"repo":root,"worktrees":serde_json::to_value(pokarak_core::worktree_inventory(&root)?)?}),
        ),
        "search" => Ok(
            json!({"repo":root,"kind":"literal_text","locations_zero_based_utf16":true,"limit":flags.number("--limit", 100, 10000)?,"matches":serde_json::to_value(pokarak_core::search(&root, flags.text("--query")?, flags.number("--limit", 100, 10000)?)?)?}),
        ),
        "read" => {
            let path = relative(&root, &flags.path("--path")?)?;
            Ok(json!({"repo":root,"path":path,"text":pokarak_core::read_file(&root,&path)?}))
        }
        "write" => {
            let path = relative(&root, &flags.path("--path")?)?;
            let expected = input_file(flags.path("--expected-file")?)?;
            let replacement = input_file(flags.path("--content-file")?)?;
            operations::save_file(&root, &path, &expected, &replacement)?;
            Ok(json!({"repo":root,"path":path,"saved":true}))
        }
        "stage" | "unstage" | "commit" => {
            let ops = GitOperations::open(&root)?;
            let snapshot = pokarak_core::index::snapshot(&root)?;
            let result = if command == "commit" {
                ops.commit_index(&snapshot, flags.text("--message")?, &CommitMode::New)?
            } else {
                let paths = flags
                    .values
                    .get("--path")
                    .context("At least one --path is required")?
                    .iter()
                    .map(|p| relative(&root, Path::new(p)))
                    .collect::<Result<Vec<_>>>()?;
                let scope = StageScope::Files(paths);
                if command == "stage" {
                    ops.stage(&snapshot, &scope)?
                } else {
                    ops.unstage(&snapshot, &scope)?
                }
            };
            Ok(output(result))
        }
        "merge" => {
            let ops = GitOperations::open(&root)?;
            let branch = flags.text("--branch")?;
            let reference = if branch.starts_with("refs/") {
                branch.into()
            } else {
                format!("refs/heads/{branch}")
            };
            let choice = ops.integration_choices()?.into_iter().find(|c| c.reference == reference).context("Branch not available for merge; use a local branch or fully qualified refs/remotes/... name")?;
            Ok(output(
                ops.integrate_branch(IntegrationKind::Merge, &choice)?,
            ))
        }
        "status" => {
            let snapshot = pokarak_core::snapshot(&root, &compare)?;
            Ok(
                json!({"repo":root,"worktree":serde_json::to_value(snapshot.worktree)?,"compare":compare,"changes":serde_json::to_value(snapshot.changes)?,"fingerprint":snapshot.fingerprint}),
            )
        }
        "files"
            if flags.switches.contains("--changed") && !flags.switches.contains("--name-only") =>
        {
            let snapshot = pokarak_core::snapshot_current(&root, &compare)?;
            Ok(
                json!({"repo":root,"compare":compare,"changes":serde_json::to_value(snapshot.changes)?}),
            )
        }
        "branches" => {
            let inventory = pokarak_core::snapshot_current_inventory(&root, &compare)?;
            Ok(json!({"repo":root,"branches":inventory.branches}))
        }
        "files" => {
            let inventory = pokarak_core::snapshot_current_inventory(&root, &compare)?;
            if flags.switches.contains("--changed") {
                let paths: Vec<_> = inventory
                    .changes
                    .into_iter()
                    .map(|file| file.path)
                    .collect();
                Ok(json!({"repo":root,"compare":compare,"files":serde_json::to_value(paths)?}))
            } else {
                Ok(json!({"repo":root,"files":serde_json::to_value(inventory.files)?}))
            }
        }
        "diff" => {
            let path = relative(&root, &flags.path("--path")?)?;
            let inventory = pokarak_core::snapshot_current_inventory(&root, &compare)?;
            let file = inventory
                .changes
                .iter()
                .find(|f| f.path == path || f.old_path.as_ref() == Some(&path))
                .context("File has no changes in the selected comparison")?;
            let doc = pokarak_core::diff(&root, file, &compare)?;
            Ok(
                json!({"repo":root,"compare":compare,"path":serde_json::to_value(doc.path)?,"patch":doc.patch,"old_text":doc.old_text,"new_text":doc.new_text,"binary":doc.binary,"truncated":doc.truncated}),
            )
        }
        _ => unreachable!(),
    }
}

fn gui(args: &[OsString]) -> Result<Value> {
    let action = args
        .first()
        .and_then(|v| v.to_str())
        .context("gui requires an action")?;
    let values: &[&str] = match action {
        "status" => &["--session"],
        "open-project" | "select-worktree" | "refresh" => &["--session", "--repo"],
        "open-file" => &["--session", "--repo", "--path", "--line"],
        "view" => &["--session", "--repo", "--mode"],
        _ => anyhow::bail!("Unknown GUI action: {action}"),
    };
    let flags = Flags::parse(&args[1..], values, &[], &[])?;
    let repo = if action == "status" {
        None
    } else {
        Some(pokarak_core::repository_root(&flags.path("--repo")?)?)
    };
    if let Some(root) = &repo {
        root.to_str()
            .context("JSON control requires a UTF-8 worktree path")?;
    }
    let path = if action == "open-file" {
        let root = repo.as_ref().unwrap();
        let path = relative(root, &flags.path("--path")?)?;
        // Validate in the CLI/background process; never perform filesystem reads on GPUI's thread.
        pokarak_core::read_file(root, &path)?;
        Some(path)
    } else {
        None
    };
    let mode = flags.optional("--mode")?.map(str::to_owned);
    if let Some(mode) = &mode {
        ensure!(
            ["graph", "code"].contains(&mode.as_str()),
            "--mode must be graph or code"
        );
    }
    let request = crate::control::Request {
        action: action.into(),
        repo,
        path,
        line: flags.number("--line", 1, u32::MAX as usize)? as u32,
        mode,
        target: None,
        activate: false,
    };
    crate::control::call(&flags.path("--session")?, &request)
}

pub fn response(args: &[OsString]) -> (Value, i32) {
    match run(args) {
        Ok(value) => (json!({"ok":true,"result":value}), 0),
        Err(error) => {
            let message = format!("{error:#}");
            let usage = [
                "required",
                "Unknown",
                "requires",
                "option",
                "--agent must",
                "--mode must",
                "must be between",
                "UTF-8",
            ]
            .iter()
            .any(|s| message.contains(s));
            (
                json!({"ok":false,"error":{"code":if usage {"usage"} else {"operation_failed"},"message":message}}),
                if usage { 2 } else { 1 },
            )
        }
    }
}
