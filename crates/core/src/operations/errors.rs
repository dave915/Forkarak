//! Actionable classifications without discarding the original error chain.
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Authentication,
    FilesystemPermission,
    RepositoryLock,
    RejectedUpdate,
    Conflict,
    Cancelled,
    Timeout,
    Other,
}

pub fn classify(error: &anyhow::Error) -> ErrorKind {
    // Typed local filesystem errors take precedence over generic Git guidance.
    if error.chain().any(|cause| cause.downcast_ref::<io::Error>()
        .is_some_and(|e| e.kind() == io::ErrorKind::PermissionDenied)) {
        return ErrorKind::FilesystemPermission;
    }
    classify_diagnostics(&format!("{error:#}"))
}

pub(crate) fn classify_diagnostics(diagnostics: &str) -> ErrorKind {
    let text = diagnostics.to_ascii_lowercase();
    if text.contains("cancelled") || text.contains("canceled") { return ErrorKind::Cancelled; }
    if text.contains("timed out") || text.contains("timeout") { return ErrorKind::Timeout; }
    if text.contains(".lock") && (text.contains("file exists") || text.contains("another git process") || text.contains("unable to create"))
        || text.contains("cannot lock ref") { return ErrorKind::RepositoryLock; }
    if ["authentication failed", "permission denied (publickey", "could not read username", "terminal prompts disabled", "invalid username or password", "http basic: access denied", "repository not found", "access denied to remote", "requested url returned error: 403", "requested url returned error: 401"].iter().any(|s| text.contains(s)) {
        return ErrorKind::Authentication;
    }
    if text.contains("permission denied") || text.contains("operation not permitted") || text.contains("read-only file system") { return ErrorKind::FilesystemPermission; }
    if text.contains("non-fast-forward") || text.contains("[rejected]") || text.contains("fetch first") { return ErrorKind::RejectedUpdate; }
    if text.contains("conflict (") || text.contains("unmerged files") || text.contains("resolve all conflicts") || text.contains("automatic merge failed") { return ErrorKind::Conflict; }
    ErrorKind::Other
}

pub fn guidance(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Authentication => "Check this remote's credentials, credential helper or SSH agent, then retry explicitly.",
        ErrorKind::FilesystemPermission => "Check local file/directory ownership and write permissions, then retry explicitly.",
        ErrorKind::RepositoryLock => "Another Git process may own this lock. Let it finish and inspect the lock before removing anything.",
        ErrorKind::RejectedUpdate => "Fetch explicitly, review the divergent commits, and choose how to integrate them before pushing again.",
        ErrorKind::Conflict => "Inspect the current operation and resolve/stage conflicts, then Continue or Abort explicitly.",
        ErrorKind::Cancelled | ErrorKind::Timeout => "Reinspect the worktree and operation state before retrying; some steps may have completed.",
        ErrorKind::Other => "Inspect the Git diagnostics and current repository state before retrying.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguish_auth_filesystem_and_lock() {
        for (text, kind) in [
            ("git@host: Permission denied (publickey).", ErrorKind::Authentication),
            ("fatal: Authentication failed for https://example.test/repo", ErrorKind::Authentication),
            ("error: unable to unlink old 'src/a': Permission denied", ErrorKind::FilesystemPermission),
            ("fatal: Unable to create '.git/index.lock': File exists.", ErrorKind::RepositoryLock),
            ("! [rejected] main -> main (non-fast-forward)", ErrorKind::RejectedUpdate),
            ("CONFLICT (content): Merge conflict in a", ErrorKind::Conflict),
        ] { assert_eq!(classify(&anyhow::anyhow!(text)), kind); }
        let e = anyhow::Error::new(io::Error::from(io::ErrorKind::PermissionDenied)).context("Cannot open file");
        assert_eq!(classify(&e), ErrorKind::FilesystemPermission);
    }
}
