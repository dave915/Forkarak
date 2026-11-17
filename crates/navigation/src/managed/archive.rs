//! Extract archives without writing through archive-provided links. All links are
//! materialized last, to existing canonical internal targets only; special files refused.
use super::{InstallControl, InstallPhase};
use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
const MAX_EXPANDED: u64 = 4 * 1024 * 1024 * 1024;
const MAX_FILE: u64 = 1024 * 1024 * 1024;

fn relative(path: &Path) -> Result<PathBuf> {
    let text = path.to_str().context("Archive path is not UTF-8")?;
    if text.contains('\\') || text.contains(':') || text.contains('\0') {
        bail!("Unsafe archive path");
    }
    let mut result = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Normal(p) => result.push(p),
            Component::CurDir => {}
            _ => bail!("Archive path traversal or absolute path refused"),
        }
    }
    if result.as_os_str().is_empty() {
        bail!("Empty archive path");
    }
    Ok(result)
}
fn internal_target(path: &Path, target: &Path, hard: bool) -> Result<PathBuf> {
    if target.is_absolute() {
        bail!("Absolute archive link refused");
    }
    let text = target.to_str().context("Non-UTF8 archive link")?;
    if text.contains('\\') || text.contains(':') {
        bail!("Unsafe archive link");
    }
    let mut normalized = if hard {
        PathBuf::new()
    } else {
        path.parent().unwrap_or(Path::new("")).to_path_buf()
    };
    for c in target.components() {
        match c {
            Component::Normal(p) => normalized.push(p),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    bail!("Archive link escapes root");
                }
            }
            _ => bail!("Unsafe archive link"),
        }
    }
    Ok(normalized)
}
fn mode(path: &Path, value: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode((value & 0o777) | 0o600))?;
    }
    Ok(())
}
fn write_entry(
    reader: &mut impl Read,
    path: &Path,
    size: u64,
    permissions: u32,
    control: &InstallControl,
) -> Result<()> {
    if size > MAX_FILE {
        bail!("Archive file exceeds size limit");
    }
    fs::create_dir_all(path.parent().context("Missing archive parent")?)?;
    // create_new refuses collisions with prior files, links or directories.
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let mut buf = [0; 64 * 1024];
    let mut count = 0;
    loop {
        control.check()?;
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > size || count > MAX_FILE {
            bail!("Archive entry exceeds declared size");
        }
        out.write_all(&buf[..n])?;
    }
    if count != size {
        bail!("Truncated archive entry");
    }
    mode(path, permissions)?;
    Ok(())
}

pub(super) fn extract(file: &Path, destination: &Path, control: &InstallControl) -> Result<()> {
    control.phase(
        InstallPhase::Extracting,
        "Validating and extracting archive",
    );
    fs::create_dir_all(destination)?;
    let mut header = [0; 4];
    let n = fs::File::open(file)?.read(&mut header)?;
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    let mut size = 0u64;
    let mut count = 0usize;
    if n >= 2 && header[..2] == *b"PK" {
        let mut zip = zip::ZipArchive::new(fs::File::open(file)?)?;
        for i in 0..zip.len() {
            control.check()?;
            count += 1;
            if count > 200_000 {
                bail!("Too many archive entries");
            }
            let mut entry = zip.by_index(i)?;
            let path = relative(Path::new(entry.name()))?;
            if !seen.insert(path.clone()) {
                bail!("Duplicate archive entry");
            }
            size = size
                .checked_add(entry.size())
                .context("Archive size overflow")?;
            if size > MAX_EXPANDED {
                bail!("Expanded archive exceeds size limit");
            }
            let permissions = entry.unix_mode().unwrap_or(0o644);
            let kind = permissions & 0o170000;
            if kind == 0o120000 {
                if entry.size() > 4096 {
                    bail!("Archive link too long");
                }
                let mut target = String::new();
                entry.read_to_string(&mut target)?;
                let target = PathBuf::from(target);
                internal_target(&path, &target, false)?;
                links.push((path, target, false));
            } else if entry.is_dir() {
                fs::create_dir_all(destination.join(path))?;
            } else {
                if !matches!(kind, 0 | 0o100000) {
                    bail!("Archive special file refused");
                }
                let declared = entry.size();
                write_entry(
                    &mut entry,
                    &destination.join(path),
                    declared,
                    permissions,
                    control,
                )?;
            }
        }
    } else {
        let mut archive = tar::Archive::new(GzDecoder::new(fs::File::open(file)?));
        for entry in archive.entries()? {
            control.check()?;
            count += 1;
            if count > 200_000 {
                bail!("Too many archive entries");
            }
            let mut entry = entry?;
            if entry.header().entry_type().is_pax_global_extensions() {
                continue;
            }
            let raw = entry.path()?.into_owned();
            if raw == Path::new(".") {
                continue;
            }
            let path = relative(&raw)?;
            let kind = entry.header().entry_type();
            if !seen.insert(path.clone()) {
                bail!("Duplicate archive entry");
            }
            let declared = entry.size();
            size = size
                .checked_add(declared)
                .context("Archive size overflow")?;
            if size > MAX_EXPANDED {
                bail!("Expanded archive exceeds size limit");
            }
            if kind.is_dir() {
                fs::create_dir_all(destination.join(path))?;
            } else if kind.is_symlink() || kind.is_hard_link() {
                let target = entry
                    .link_name()?
                    .context("Missing archive link target")?
                    .into_owned();
                internal_target(&path, &target, kind.is_hard_link())?;
                links.push((path, target, kind.is_hard_link()));
            } else if kind.is_file() {
                let permissions = entry.header().mode()?;
                write_entry(
                    &mut entry,
                    &destination.join(path),
                    declared,
                    permissions,
                    control,
                )?;
            } else {
                bail!("Archive special file refused");
            }
        }
    }
    let root = destination.canonicalize()?;
    while !links.is_empty() {
        let old = links.len();
        let mut remaining = Vec::new();
        for (path, target, hard) in links {
            control.check()?;
            let normalized = internal_target(&path, &target, hard)?;
            let source = destination.join(normalized);
            let real = match source.canonicalize() {
                Ok(p) => p,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    remaining.push((path, target, hard));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            if !real.starts_with(&root) {
                bail!("Archive link resolves outside root");
            }
            let out = destination.join(&path);
            fs::create_dir_all(out.parent().context("Missing link parent")?)?;
            if !out.parent().unwrap().canonicalize()?.starts_with(&root) {
                bail!("Archive link parent escapes root");
            }
            // Normalize links to copies so payload paths remain safe after staging rename,
            // macOS signing, and recursive traversal. Directory links may be internal, but
            // copy recursively would permit cycles; use validated relative symlinks there.
            if hard {
                fs::hard_link(&real, &out)?;
            } else {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, &out)?;
                #[cfg(not(unix))]
                {
                    if real.is_file() {
                        fs::copy(real, out)?;
                    } else {
                        bail!("Directory symlink unsupported on this platform");
                    }
                }
            }
        }
        if remaining.len() == old {
            bail!("Unresolved or cyclic archive links refused");
        }
        links = remaining;
    }
    Ok(())
}
pub(super) fn executable(path: &Path) -> Result<()> {
    mode(path, 0o755)
}
pub(super) fn gunzip(file: &Path, path: &Path, control: &InstallControl) -> Result<()> {
    let mut input = GzDecoder::new(fs::File::open(file)?);
    let mut output = fs::File::create(path)?;
    let mut buf = [0; 64 * 1024];
    let mut total = 0;
    loop {
        control.check()?;
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_FILE {
            bail!("Expanded binary exceeds size limit");
        }
        output.write_all(&buf[..n])?;
    }
    executable(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_path_and_link_escapes() {
        for p in ["../escape", "/escape", "C:/escape", "a/../../b", "a\\b"] {
            assert!(relative(Path::new(p)).is_err(), "{p}");
        }
        assert!(internal_target(Path::new("a/link"), Path::new("../../escape"), false).is_err());
        assert_eq!(
            internal_target(Path::new("a/link"), Path::new("../lib/file"), false).unwrap(),
            Path::new("lib/file")
        );
    }
    fn tar_link(target: &str) -> Vec<u8> {
        let out = Vec::new();
        let gzip = flate2::write::GzEncoder::new(out, flate2::Compression::default());
        let mut tar = tar::Builder::new(gzip);
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o644);
        h.set_size(2);
        h.set_cksum();
        tar.append_data(&mut h, "lib/file", &b"ok"[..]).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_mode(0o755);
        h.set_size(0);
        h.set_link_name(target).unwrap();
        h.set_cksum();
        tar.append_data(&mut h, "bin/link", std::io::empty())
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }
    #[test]
    fn safe_internal_links_and_escape_refusal() {
        for (target, good) in [
            ("../lib/file", true),
            ("../../escape", false),
            ("/etc/passwd", false),
            ("link", false),
        ] {
            let t = tempfile::tempdir().unwrap();
            let archive = t.path().join("a.tgz");
            fs::write(&archive, tar_link(target)).unwrap();
            let dest = t.path().join("out");
            let result = extract(&archive, &dest, &InstallControl::new("test"));
            assert_eq!(result.is_ok(), good, "{target}: {result:?}");
            if good {
                assert_eq!(fs::read(dest.join("bin/link")).unwrap(), b"ok");
            }
        }
    }
    #[test]
    fn authored_malicious_tar_cannot_traverse_or_write_special_files() {
        for (name, kind) in [
            ("../escape", tar::EntryType::Regular),
            ("/absolute", tar::EntryType::Regular),
            ("device", tar::EntryType::Fifo),
        ] {
            let t = tempfile::tempdir().unwrap();
            let mut gzip =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            {
                let mut tar = tar::Builder::new(&mut gzip);
                let mut header = tar::Header::new_gnu();
                header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
                header.set_size(0);
                header.set_mode(0o644);
                header.set_entry_type(kind);
                header.set_cksum();
                tar.append(&header, std::io::empty()).unwrap();
                tar.finish().unwrap();
            }
            let file = t.path().join("malicious.tgz");
            fs::write(&file, gzip.finish().unwrap()).unwrap();
            assert!(extract(&file, &t.path().join("out"), &InstallControl::new("test")).is_err());
            assert!(!t.path().join("escape").exists());
        }
    }
    #[test]
    fn cancellation_refuses_extraction() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("a.tgz");
        fs::write(&file, tar_link("../lib/file")).unwrap();
        let control = InstallControl::new("test");
        control.cancel().unwrap();
        assert!(extract(&file, &t.path().join("out"), &control).is_err());
    }
}
