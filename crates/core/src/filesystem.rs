//! Descriptor-relative, no-follow file access. In particular a tracked parent
//! directory replaced by a symlink can never expose files outside the worktree.
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fmt,
    fs::File,
    io::{self, Read},
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};

use crate::{MAX_TEXT_BYTES, eol, git::Entry};

pub(crate) enum Node {
    Missing,
    File(File),
    Link(Vec<u8>),
    Directory,
}

/// Typed context only for permission failures on a verified regular file.
/// Repository, ancestor directory and special-file errors remain fatal.
#[derive(Debug)]
pub(crate) struct UnreadableFile(pub PathBuf);

impl fmt::Display for UnreadableFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cannot read regular file {}", self.0.display())
    }
}

fn regular_io<T>(path: &Path, result: io::Result<T>) -> Result<T> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            Err(error).context(UnreadableFile(path.to_owned()))
        }
        result => Ok(result?),
    }
}

fn is_regular_at(directory: &File, name: &CString) -> bool {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the directory and name are live, and fstatat initializes stat on success.
    unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        ) == 0
            && stat.assume_init().st_mode & libc::S_IFMT == libc::S_IFREG
    }
}

pub(crate) fn relative(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.strip_prefix(root)
            .context("Path is outside the selected worktree")?
    } else {
        path
    };
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                ensure!(
                    !name.as_bytes().eq_ignore_ascii_case(b".git"),
                    "Reading Git administration paths is not allowed"
                );
                ensure!(
                    !name.as_bytes().contains(&0),
                    "A file path cannot contain NUL"
                );
                result.push(name);
            }
            Component::CurDir => (),
            _ => bail!("Path traversal is not allowed; select a file within the worktree"),
        }
    }
    ensure!(
        !result.as_os_str().is_empty(),
        "Select a file, not the worktree directory"
    );
    Ok(result)
}

pub(crate) fn open(root: &Path, path: &Path) -> Result<Node> {
    let relative = relative(root, path)?;
    let components = relative.components().collect::<Vec<_>>();
    let mut directory = File::open(root).context("Cannot open worktree directory")?;
    for (i, component) in components.iter().enumerate() {
        let name = CString::new(component.as_os_str().as_bytes())?;
        let last = i + 1 == components.len();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if last { 0 } else { libc::O_DIRECTORY };
        // SAFETY: directory and name remain valid for the call; a successful
        // descriptor is uniquely owned by the File created immediately below.
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            let error = io::Error::last_os_error();
            if last
                && error.kind() == io::ErrorKind::PermissionDenied
                && is_regular_at(&directory, &name)
            {
                return Err(error).context(UnreadableFile(relative));
            }
            if last && error.raw_os_error() == Some(libc::ELOOP) {
                let mut target = vec![0; 64 * 1024];
                // SAFETY: the buffer is writable for its full capacity, and no
                // path is followed by readlinkat, even if it changes concurrently.
                let n = unsafe {
                    libc::readlinkat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        target.as_mut_ptr().cast(),
                        target.len(),
                    )
                };
                if n < 0 {
                    return Err(io::Error::last_os_error())
                        .context("Symlink changed while reading; refresh and retry");
                }
                ensure!(
                    (n as usize) < target.len(),
                    "Symlink target exceeds the safety limit"
                );
                target.truncate(n as usize);
                return Ok(Node::Link(target));
            }
            if matches!(
                error.raw_os_error(),
                Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP)
            ) {
                return Ok(Node::Missing);
            }
            return Err(error)
                .with_context(|| format!("Cannot safely open {}", relative.display()));
        }
        // SAFETY: openat returned a new, owned descriptor.
        let file = unsafe { File::from_raw_fd(descriptor) };
        if last {
            let metadata = file.metadata()?;
            return if metadata.is_file() {
                Ok(Node::File(file))
            } else if metadata.is_dir() {
                Ok(Node::Directory)
            } else {
                bail!(
                    "{} is not a regular file or symlink; special files cannot be viewed",
                    relative.display()
                )
            };
        }
        directory = file;
    }
    unreachable!("relative paths always have a component")
}

enum BlobHash {
    Sha1(sha1::Sha1),
    Sha256(Sha256),
}
impl BlobHash {
    fn new(sha256: bool, length: u64) -> Self {
        let mut hash = if sha256 {
            Self::Sha256(Sha256::new())
        } else {
            Self::Sha1(sha1::Sha1::new())
        };
        hash.update(format!("blob {length}\0").as_bytes());
        hash
    }
    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Sha1(h) => h.update(bytes),
            Self::Sha256(h) => h.update(bytes),
        }
    }
    fn finish(self) -> String {
        match self {
            Self::Sha1(h) => format!("{:x}", h.finalize()),
            Self::Sha256(h) => format!("{:x}", h.finalize()),
        }
    }
}

pub(crate) fn inspect(
    root: &Path,
    path: &Path,
    sha256: bool,
    filemode: bool,
) -> Result<Option<(Entry, Option<eol::Stats>)>> {
    match open(root, path)? {
        Node::Missing | Node::Directory => Ok(None),
        Node::Link(target) => {
            let mut hash = BlobHash::new(sha256, target.len() as u64);
            hash.update(&target);
            Ok(Some((
                Entry {
                    mode: "120000".into(),
                    oid: hash.finish(),
                },
                None,
            )))
        }
        Node::File(mut file) => {
            let metadata = regular_io(path, file.metadata())?;
            let mut hash = BlobHash::new(sha256, metadata.len());
            let mut buffer = [0u8; 64 * 1024];
            let mut length = 0u64;
            let mut eol = eol::Stats::default();
            loop {
                let n = regular_io(path, file.read(&mut buffer))?;
                if n == 0 {
                    break;
                }
                length += n as u64;
                hash.update(&buffer[..n]);
                // Most source files fit in one buffer and contain only LF.
                if length != n as u64 || length < metadata.len() || buffer[..n].contains(&b'\r') {
                    eol.update(&buffer[..n]);
                }
            }
            let after = regular_io(path, file.metadata())?;
            ensure!(
                length == metadata.len()
                    && metadata.len() == after.len()
                    && metadata.modified()? == after.modified()?,
                "{} changed while reading; refresh and retry",
                path.display()
            );
            let mode = if filemode && metadata.permissions().mode() & 0o111 != 0 {
                "100755"
            } else {
                "100644"
            };
            Ok(Some((
                Entry {
                    mode: mode.into(),
                    oid: hash.finish(),
                },
                eol.has_crlf().then_some(eol),
            )))
        }
    }
}

/// Git's blob header includes the cleaned length. The first pass counted CRLF
/// pairs, and its raw OID guards this second pass against concurrent edits.
pub(crate) fn normalized_oid(
    root: &Path,
    path: &Path,
    sha256: bool,
    length: u64,
    expected: &str,
) -> Result<String> {
    let Node::File(mut file) = open(root, path)? else {
        bail!(
            "{} changed while reading; refresh and retry",
            path.display()
        );
    };
    let mut raw = BlobHash::new(sha256, regular_io(path, file.metadata())?.len());
    let mut normalized = BlobHash::new(sha256, length);
    let mut buffer = [0; 64 * 1024];
    let mut carry = 0;
    loop {
        let n = regular_io(path, file.read(&mut buffer[carry..]))?;
        if n == 0 {
            normalized.update(&buffer[..carry]);
            break;
        }
        let end = carry + n;
        raw.update(&buffer[carry..end]);
        carry = usize::from(buffer[end - 1] == b'\r');
        let written = eol::normalize(&mut buffer[..end - carry]);
        normalized.update(&buffer[..written]);
        if carry != 0 {
            buffer[0] = b'\r';
        }
    }
    ensure!(
        raw.finish() == expected,
        "{} changed while reading; refresh and retry",
        path.display()
    );
    Ok(normalized.finish())
}

pub(crate) fn preview(path: &Path, file: File) -> Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    regular_io(
        path,
        file.take(MAX_TEXT_BYTES as u64 + 1).read_to_end(&mut bytes),
    )?;
    let truncated = bytes.len() > MAX_TEXT_BYTES;
    bytes.truncate(MAX_TEXT_BYTES);
    Ok((bytes, truncated))
}

pub(crate) fn read(root: &Path, path: &Path) -> Result<Option<(Vec<u8>, bool)>> {
    match open(root, path)? {
        Node::Missing | Node::Directory => Ok(None),
        Node::Link(bytes) => Ok(Some((bytes, false))),
        Node::File(file) => preview(path, file).map(Some),
    }
}

/// Length-framed values avoid collisions from ambiguous path/content joins.
pub(crate) fn feed(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
