use super::{Entry, blob, spawn_read};
use crate::MAX_TEXT_BYTES;
use anyhow::{Context, Result, ensure};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdout, Stdio},
    thread::JoinHandle,
};

/// Request one object at a time, so neither pipe can fill while waiting for
/// the other. Only object IDs from the baseline tree go through this protocol.
pub(crate) struct BlobBatch<'a> {
    root: &'a Path,
    session: Option<Session>,
}

impl<'a> BlobBatch<'a> {
    pub(crate) fn new(root: &'a Path) -> Self {
        Self {
            root,
            session: None,
        }
    }

    pub(crate) fn read(&mut self, entry: &Entry) -> Result<(Vec<u8>, bool)> {
        if entry.mode == "160000" {
            return blob(self.root, entry); // synthetic submodule text
        }
        if self.session.is_none() {
            self.session = Some(Session::start(self.root)?);
        }
        let session = self.session.as_mut().expect("batch session started");
        let (bytes, truncated) = session.read(&entry.oid).with_context(|| {
            format!(
                "Cannot read Git blob {} (fetch missing objects explicitly)",
                entry.oid
            )
        })?;
        if truncated {
            // Do not drain a potentially enormous object just to reach the next
            // header. The next request starts a fresh batch at an object boundary.
            self.session = None;
        }
        Ok((bytes, truncated))
    }
}

struct Session {
    child: Child,
    output: BufReader<ChildStdout>,
    errors: Option<JoinHandle<Vec<u8>>>,
}

impl Session {
    fn start(root: &Path) -> Result<Self> {
        let (mut child, errors) = spawn_read(
            root,
            &["cat-file".as_ref(), "--batch".as_ref()],
            Stdio::piped(),
        )?;
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        Ok(Self {
            child,
            output,
            errors: Some(errors),
        })
    }

    fn read(&mut self, oid: &str) -> Result<(Vec<u8>, bool)> {
        writeln!(self.child.stdin.as_mut().expect("piped stdin"), "{oid}")?;
        let mut header = String::new();
        self.output.read_line(&mut header)?;
        let size: u64 = header
            .strip_prefix(oid)
            .and_then(|tail| tail.strip_prefix(" blob "))
            .and_then(|size| size.strip_suffix('\n'))
            .with_context(|| format!("Expected a blob, got {header:?}"))?
            .parse()?;
        let mut bytes = vec![0; size.min(MAX_TEXT_BYTES as u64) as usize];
        self.output.read_exact(&mut bytes)?;
        let truncated = size > MAX_TEXT_BYTES as u64;
        if !truncated {
            let mut delimiter = [0];
            self.output.read_exact(&mut delimiter)?;
            ensure!(delimiter[0] == b'\n', "Missing cat-file batch delimiter");
        }
        Ok((bytes, truncated))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Also reaps the child on a failed request or partially consumed blob.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(errors) = self.errors.take() {
            let _ = errors.join();
        }
    }
}
