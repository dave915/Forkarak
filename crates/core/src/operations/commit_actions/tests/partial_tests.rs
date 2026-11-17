use super::*;
use crate::{
    DiffKind,
    index::partial::{self, PartialPreview, Side},
};

fn preview(repo: &Repo, path: &str, side: Side) -> PartialPreview {
    partial::preview(&repo.snap(), Path::new(path), side).unwrap()
}

fn row(preview: &PartialPreview, kind: DiffKind, text: &str) -> usize {
    preview
        .lines()
        .iter()
        .position(|r| r.kind == kind && r.text == text)
        .unwrap()
}

#[test]
fn stage_and_unstage_selected_hunks_and_lines_preserves_other_changes() {
    let repo = Repo::new();
    let base = (1..=23).map(|n| format!("line {n}\n")).collect::<String>();
    repo.write("file", &base);
    repo.write("other", "old\n");
    repo.stage_all();
    repo.op()
        .commit_index(&repo.snap(), "initial", &CommitMode::New)
        .unwrap();
    let first = base.replace("line 2\n", "line two\n");
    let working = first.replace("line 22\n", "line twenty two\n");
    repo.write("file", &working);
    repo.write("other", "independent\n");
    repo.op().stage(&repo.snap(), &files(&["other"])).unwrap();

    let initial = preview(&repo, "file", Side::Unstaged);
    assert_eq!(
        initial
            .lines()
            .iter()
            .filter(|r| r.text.starts_with("@@ "))
            .count(),
        2
    );
    let first_hunk = initial.hunk_lines(0);
    assert_eq!(first_hunk.len(), 2);
    repo.op()
        .apply_selected_lines(&initial, &first_hunk)
        .unwrap();
    assert_eq!(repo.git(&["show", ":file"]), first.trim_end());
    let unstaged = preview(&repo, "file", Side::Unstaged);
    repo.op()
        .apply_selected_lines(
            &unstaged,
            &[row(&unstaged, DiffKind::Addition, "line twenty two")],
        )
        .unwrap();
    let both = first.replace("line 22\n", "line 22\nline twenty two\n");
    assert_eq!(repo.git(&["show", ":file"]), both.trim_end());
    let staged = preview(&repo, "file", Side::Staged);
    repo.op()
        .apply_selected_lines(&staged, &staged.hunk_lines(0))
        .unwrap();
    assert_eq!(
        repo.git(&["show", ":file"]),
        base.replace("line 22\n", "line 22\nline twenty two\n")
            .trim_end()
    );
    assert_eq!(repo.git(&["show", ":other"]), "independent");
    assert_eq!(fs::read_to_string(repo.root.join("file")).unwrap(), working);
}

#[test]
fn partial_new_deleted_unborn_literal_paths_modes_and_exact_terminators() {
    let repo = Repo::new();
    let name = "-leading [abc] 한.txt";
    repo.write(name, "first\r\nsecond");
    fs::set_permissions(repo.root.join(name), fs::Permissions::from_mode(0o755)).unwrap();
    let p = preview(&repo, name, Side::Unstaged);
    repo.op()
        .apply_selected_lines(&p, &[row(&p, DiffKind::Addition, "first")])
        .unwrap();
    let read = |repo: &Repo| index::diff(&repo.snap(), Path::new(name)).unwrap().new_text;
    assert_eq!(read(&repo), "first\r\n");
    let p = preview(&repo, name, Side::Unstaged);
    repo.op()
        .apply_selected_lines(&p, &p.hunk_lines(0))
        .unwrap();
    assert_eq!(read(&repo), "first\r\nsecond");
    assert!(
        repo.git(&["ls-files", "--stage", "--", name])
            .starts_with("100755 ")
    );
    let p = preview(&repo, name, Side::Staged);
    repo.op()
        .apply_selected_lines(&p, &p.hunk_lines(0))
        .unwrap();
    assert!(repo.git(&["ls-files", "--", name]).is_empty());
    assert_eq!(fs::read(repo.root.join(name)).unwrap(), b"first\r\nsecond");

    repo.stage_all();
    repo.op()
        .commit_index(&repo.snap(), "new file", &CommitMode::New)
        .unwrap();
    fs::remove_file(repo.root.join(name)).unwrap();
    let p = preview(&repo, name, Side::Unstaged);
    repo.op()
        .apply_selected_lines(&p, &[row(&p, DiffKind::Deletion, "second")])
        .unwrap();
    assert_eq!(read(&repo), "first\r\n");
    let p = preview(&repo, name, Side::Unstaged);
    repo.op()
        .apply_selected_lines(&p, &p.hunk_lines(0))
        .unwrap();
    assert!(repo.git(&["ls-files", "--", name]).is_empty());
    let p = preview(&repo, name, Side::Staged);
    repo.op()
        .apply_selected_lines(&p, &[row(&p, DiffKind::Deletion, "first")])
        .unwrap();
    assert_eq!(read(&repo), "first\r\n");
    assert!(!repo.root.join(name).exists());
}

#[test]
fn partial_preview_refuses_stale_inputs_wrong_root_lock_and_non_changes() {
    let repo = Repo::new();
    repo.seed();
    repo.write("file", "new\nline\n");
    let p = preview(&repo, "file", Side::Unstaged);
    let rows = p.hunk_lines(0);
    let other = Repo::new();
    assert!(other.op().apply_selected_lines(&p, &rows).is_err());
    assert!(repo.op().apply_selected_lines(&p, &[0]).is_err());
    assert!(repo.op().apply_selected_lines(&p, &[usize::MAX]).is_err());
    repo.write(".git/index.lock", "other owner\n");
    assert!(repo.op().apply_selected_lines(&p, &rows).is_err());
    assert_eq!(
        fs::read(repo.root.join(".git/index.lock")).unwrap(),
        b"other owner\n"
    );
    fs::remove_file(repo.root.join(".git/index.lock")).unwrap();
    repo.write("file", "late edit");
    assert!(repo.op().apply_selected_lines(&p, &rows).is_err());
    repo.write("file", "new\nline\n");
    repo.write("extra", "staged independently");
    repo.git(&["add", "--", "extra"]);
    assert!(repo.op().apply_selected_lines(&p, &rows).is_err());
    assert_eq!(repo.git(&["show", ":file"]), "base");
    assert_eq!(repo.git(&["show", ":extra"]), "staged independently");
    assert!(!repo.root.join(".git/index.lock").exists());
}

#[test]
fn partial_preview_never_runs_filters_and_rejects_special_files() {
    let repo = Repo::new();
    repo.seed();
    repo.write("file", "new\n");
    repo.write(".gitattributes", "file filter=authored\n");
    repo.git(&["config", "filter.authored.clean", "touch forbidden; cat"]);
    let snap = repo.snap();
    assert!(partial::preview(&snap, Path::new("file"), Side::Unstaged).is_err());
    assert!(!repo.root.join("forbidden").exists());
    fs::remove_file(repo.root.join(".gitattributes")).unwrap();
    repo.write("file", b"a\x00b");
    assert!(partial::preview(&repo.snap(), Path::new("file"), Side::Unstaged).is_err());
    fs::remove_file(repo.root.join("file")).unwrap();
    symlink("elsewhere", repo.root.join("file")).unwrap();
    assert!(partial::preview(&repo.snap(), Path::new("file"), Side::Unstaged).is_err());
}

#[test]
fn partial_staging_uses_linked_worktrees_split_index() {
    let repo = Repo::new();
    repo.seed();
    let linked = repo._temp.path().join("linked");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        "--",
        linked.to_str().unwrap(),
    ]);
    let out = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", &repo.global)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .arg("-C")
        .arg(&linked)
        .args(["update-index", "--split-index"])
        .output()
        .unwrap();
    assert!(out.status.success());
    fs::write(linked.join("file"), "linked\nbase\n").unwrap();
    let snap = index::snapshot(&linked).unwrap();
    let p = partial::preview(&snap, Path::new("file"), Side::Unstaged).unwrap();
    let mut op = GitOperations::open(&linked).unwrap();
    op.global_config = Some(repo.global.clone());
    op.apply_selected_lines(&p, &p.hunk_lines(0)).unwrap();
    assert_eq!(
        index::diff(&index::snapshot(&linked).unwrap(), Path::new("file"))
            .unwrap()
            .new_text,
        "linked\nbase\n"
    );
    assert_eq!(repo.git(&["show", ":file"]), "base");
}
