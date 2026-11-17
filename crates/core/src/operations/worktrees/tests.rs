use super::super::worktrees::WorktreeBranch;
use crate::{Worktree, git};

fn row(op: &GitOperations, branch: &str) -> Worktree {
    git::parse_worktrees(
        &op.metadata(&["worktree", "list", "--porcelain", "-z"])
            .unwrap(),
    )
    .into_iter()
    .find(|w| w.branch == branch)
    .unwrap()
}

#[test]
fn create_at_head_or_existing_branch_and_remove_only_selected_registration() {
    let f = Fixture::new();
    let op = f.op();
    let initial = f.git(&["rev-parse", "HEAD"]);
    f.git(&["branch", "older"]);
    f.commit("extra", "second");
    let head = f.git(&["rev-parse", "HEAD"]);
    let old = f.temp.path().join("folder with spaces");
    let new = f.temp.path().join("parent").join("new");
    op.create_worktree(&old, &WorktreeBranch::Existing("older".into()), &head)
        .unwrap();
    let shown = row(&op, "older");
    assert_eq!(shown.head, initial);
    op.create_worktree(&new, &WorktreeBranch::New("topic/new".into()), &head)
        .unwrap();
    assert_eq!(
        f.at(&new, &["symbolic-ref", "--short", "HEAD"]),
        "topic/new"
    );
    assert_eq!(row(&op, "topic/new").head, head);
    assert_eq!(f.git(&["symbolic-ref", "--short", "HEAD"]), "main");
    op.remove_worktree(&shown).unwrap();
    assert!(!old.exists());
    assert_eq!(f.git(&["rev-parse", "refs/heads/older"]), initial);
    let new_row = row(&op, "topic/new");
    fs::remove_dir_all(&new).unwrap();
    assert!(op.remove_worktree(&new_row).is_err());
    let gone = row(&op, "topic/new");
    assert!(gone.is_prunable);
    op.remove_worktree(&gone).unwrap();
    assert_eq!(f.git(&["rev-parse", "refs/heads/topic/new"]), head);
}

#[test]
fn protected_and_stale_worktrees_survive_removal() {
    let f = Fixture::new();
    let op = f.op();
    let linked = f.temp.path().join("linked");
    let main = row(&op, "main");
    let new = WorktreeBranch::New("linked".into());
    op.create_worktree(&linked, &new, &main.head).unwrap();
    let selected = row(&op, "linked");
    let linked_op = GitOperations {
        root: linked.canonicalize().unwrap(),
        ..op.clone()
    };
    assert!(op.remove_worktree(&main).is_err());
    assert!(linked_op.remove_worktree(&selected).is_err());
    assert!(linked_op.remove_worktree(&main).is_err());
    f.git(&["worktree", "lock", "--", linked.to_str().unwrap()]);
    assert!(op.remove_worktree(&selected).is_err());
    f.git(&["worktree", "unlock", "--", linked.to_str().unwrap()]);
    fs::write(linked.join("untracked"), "data").unwrap();
    assert!(op.remove_worktree(&selected).is_err());
    fs::write(f.root.join(".git/info/exclude"), "untracked\n").unwrap();
    assert!(op.remove_worktree(&selected).is_err());
    assert_eq!(
        fs::read_to_string(linked.join("untracked")).unwrap(),
        "data"
    );
    fs::remove_file(linked.join("untracked")).unwrap();
    fs::write(linked.join("tracked.txt"), "modified").unwrap();
    assert!(op.remove_worktree(&selected).is_err());
    f.at(
        &linked,
        &["update-index", "--assume-unchanged", "--", "tracked.txt"],
    );
    assert!(op.remove_worktree(&selected).is_err());
    f.at(
        &linked,
        &["update-index", "--no-assume-unchanged", "--", "tracked.txt"],
    );
    f.at(&linked, &["add", "--", "tracked.txt"]);
    assert!(op.remove_worktree(&selected).is_err());
    f.at(&linked, &["commit", "-m", "linked updated"]);
    assert!(op.remove_worktree(&selected).is_err());
    op.remove_worktree(&row(&op, "linked")).unwrap();
}

#[test]
fn create_rejects_stale_boundaries_before_creating_a_branch() {
    let f = Fixture::new();
    let op = f.op();
    let main = row(&op, "main");
    let dest = f.temp.path().join("dest");
    f.commit("second", "new head");
    let new = WorktreeBranch::New("new".into());
    assert!(op.create_worktree(&dest, &new, &main.head).is_err());
    let head = row(&op, "main").head;
    assert!(
        op.create_worktree(&dest, &WorktreeBranch::Existing("main".into()), &head)
            .is_err()
    );
    assert!(
        op.create_worktree(&dest, &WorktreeBranch::New("--help".into()), &head)
            .is_err()
    );
    assert!(
        op.create_worktree(&f.root.join(".git/private"), &new, &head)
            .is_err()
    );
    assert!(!dest.exists());
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("keep"), "local").unwrap();
    assert!(op.create_worktree(&dest, &new, &head).is_err());
    assert_eq!(fs::read_to_string(dest.join("keep")).unwrap(), "local");
    assert_eq!(
        f.git(&["for-each-ref", "--format=%(refname)", "refs/heads/"]),
        "refs/heads/main"
    );
}
