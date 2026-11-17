//! Explicit single-tag creation and push. No implicit tags or forced updates.
use super::*;
impl GitOperations {
    pub fn create_tag(
        &self,
        name: &str,
        target: &str,
        message: Option<&str>,
    ) -> Result<OperationOutput> {
        self.validate_tag(name)?;
        // Resolve now to an immutable commit, never let an ambiguous short ref
        // or a moving branch silently select a different creation target.
        let oid = crate::comparison::resolve(&self.root, target)?;
        let mut command = self.command(&["-c", "tag.gpgSign=false", "tag"]);
        if let Some(message) = message {
            ensure!(
                !message.trim().is_empty() && message.len() <= 64 * 1024 && !message.contains('\0'),
                "Enter a tag message up to 64 KiB"
            );
            command.args(["--annotate", "--no-sign", "--message", message]);
        }
        command.args(["--", name, &oid]);
        self.workflow_output(self.check_result(self.run_command(command)?)?)
    }
    pub fn push_tag(&self, remote: &str, name: &str) -> Result<OperationOutput> {
        self.validate_tag(name)?;
        self.require_remote(remote, true)?;
        let reference = format!("refs/tags/{name}");
        self.metadata(&["show-ref", "--verify", "--", &reference])?;
        let spec = format!("{reference}:{reference}");
        let mirror = format!("remote.{remote}.mirror=false");
        self.workflow_output(self.checked(&[
            "-c",
            &mirror,
            "push",
            "--progress",
            "--no-force",
            "--no-follow-tags",
            "--no-prune",
            "--recurse-submodules=no",
            "--",
            remote,
            &spec,
        ])?)
    }
    fn validate_tag(&self, name: &str) -> Result<()> {
        ensure!(
            !name.is_empty() && name.len() <= 1024 && !name.starts_with('-') && name != "HEAD",
            "Enter a tag name"
        );
        let reference = format!("refs/tags/{name}");
        ensure!(
            self.run(&["check-ref-format", &reference])?.code == Some(0),
            "Invalid tag name"
        );
        Ok(())
    }
}
