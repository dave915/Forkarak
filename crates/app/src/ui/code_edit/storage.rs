//! Each window remembers the exact revision it last loaded or checkpointed.
//! Only that revision may be retired. A concurrent replacement has a fresh ID.
use super::{Draft, FileKey};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Default, Serialize, Deserialize)]
struct Archive {
    // Keep the counter even when the last draft is acknowledged. A stale
    // window must never mistake a later, byte-identical draft for its own.
    last_revision: u64,
    versions: BTreeMap<u64, Draft>,
}

impl Archive {
    fn read(path: &Path) -> io::Result<Self> {
        let bytes = match fs::read(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            result => result?,
        };
        let archive = if bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'[') {
            let mut archive = Self::default();
            // File order gives every process the same initial revision IDs.
            for draft in serde_json::from_slice::<Vec<Draft>>(&bytes)? {
                validate_key(&draft)?;
                if draft.expected != draft.text {
                    archive.add(draft)?;
                }
            }
            archive
        } else {
            serde_json::from_slice(&bytes)?
        };
        for (&revision, draft) in &archive.versions {
            validate_key(draft)?;
            if revision == 0 || revision > archive.last_revision || draft.text == draft.expected {
                return Err(io::Error::other("잘못된 편집 초안 버전입니다."));
            }
        }
        Ok(archive)
    }

    fn add(&mut self, draft: Draft) -> io::Result<u64> {
        let id = self
            .last_revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("초안 버전 번호를 할당할 수 없습니다."))?;
        self.last_revision = id;
        self.versions.insert(id, draft);
        Ok(id)
    }

    fn write(&self, path: &Path) -> io::Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        serde_json::to_writer(temporary.as_file_mut(), self)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path)?;
        Ok(())
    }

    fn alternatives(&self, selected: &Selected) -> Alternatives {
        let mut choices = Alternatives::new();
        for (&revision, draft) in self.versions.iter().rev() {
            if selected
                .get(&draft.key)
                .is_some_and(|v| v.revision == revision)
            {
                continue;
            }
            let preview = draft
                .text
                .lines()
                .map(str::trim)
                .find(|s| !s.is_empty())
                .unwrap_or_default()
                .chars()
                .take(48)
                .collect();
            choices
                .entry(draft.key.clone())
                .or_default()
                .push(Choice { revision, preview });
        }
        choices
    }
}

fn validate_key(draft: &Draft) -> io::Result<()> {
    if FileKey::new(&draft.key.root, &draft.key.path).as_ref() != Some(&draft.key) {
        return Err(io::Error::other("잘못된 경로가 있는 편집 초안 파일입니다."));
    }
    Ok(())
}

pub(super) struct Choice {
    pub(super) revision: u64,
    pub(super) preview: String,
}
pub(super) type Alternatives = HashMap<FileKey, Vec<Choice>>;

#[derive(Clone)]
struct Observed {
    revision: u64,
    draft: Arc<Draft>,
}
type Selected = HashMap<FileKey, Observed>;

#[derive(Default)]
struct Writer {
    generation: u64,
    selected: Selected,
}

#[derive(Default)]
pub(super) struct Checkpoint {
    pub(super) alternatives: Alternatives,
    pub(super) recovered: Option<Draft>,
    pub(super) retired: HashSet<FileKey>,
}

pub(super) struct DraftStorage {
    pub(super) path: PathBuf,
    writer: Mutex<Writer>,
}

impl DraftStorage {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            writer: Mutex::default(),
        }
    }

    pub(super) fn load(&mut self) -> io::Result<(Vec<Draft>, Alternatives)> {
        // Atomic replacement makes a single read a consistent startup snapshot.
        let archive = Archive::read(&self.path)?;
        let writer = self.writer.get_mut().unwrap();
        for (&revision, draft) in &archive.versions {
            writer.selected.insert(
                draft.key.clone(),
                Observed {
                    revision,
                    draft: Arc::new(draft.clone()),
                },
            );
        }
        let active = writer
            .selected
            .values()
            .map(|v| (*v.draft).clone())
            .collect();
        Ok((active, archive.alternatives(&writer.selected)))
    }

    pub(super) fn checkpoint(
        &self,
        generation: u64,
        drafts: &[Draft],
        recover: Option<(&FileKey, u64)>,
    ) -> io::Result<Checkpoint> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("초안 저장 잠금 오류"))?;
        if generation < writer.generation {
            return Ok(Checkpoint::default());
        }
        writer.generation = generation;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("초안 저장 경로 없음"))?;
        fs::create_dir_all(parent)?;
        // Lock a stable sibling. Renaming the JSON while locking the JSON's
        // inode would allow another process to enter through its replacement.
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path.with_extension("lock"))?;
        lock.lock()?;
        let mut archive = Archive::read(&self.path)?;
        let before = archive.last_revision;
        let mut selected = Selected::new();
        for draft in drafts {
            let observed = match writer.selected.get(&draft.key) {
                // Keep even an externally retired baseline in this window.
                // An unchanged refresh/close must never publish it again.
                Some(base) if base.draft.as_ref() == draft => base.clone(),
                _ => Observed {
                    revision: archive.add(draft.clone())?,
                    draft: Arc::new(draft.clone()),
                },
            };
            selected.insert(draft.key.clone(), observed);
        }
        let mut changed = before != archive.last_revision;
        for (key, base) in &writer.selected {
            if selected
                .get(key)
                .is_none_or(|v| v.revision != base.revision)
            {
                changed |= archive.versions.remove(&base.revision).is_some();
            }
        }
        if changed {
            archive.write(&self.path)?;
        }
        // The picker refers to an immutable revision. A version saved or
        // discarded since the menu opened is no longer eligible for recovery.
        let mut recovered = None;
        if let Some((key, revision)) = recover
            && let Some(draft) = archive.versions.get(&revision).filter(|d| &d.key == key)
        {
            selected.insert(
                key.clone(),
                Observed {
                    revision,
                    draft: Arc::new(draft.clone()),
                },
            );
            recovered = Some(draft.clone());
        }
        let retired = selected
            .iter()
            .filter(|(_, v)| !archive.versions.contains_key(&v.revision))
            .map(|(key, _)| key.clone())
            .collect();
        writer.selected = selected;
        Ok(Checkpoint {
            alternatives: archive.alternatives(&writer.selected),
            recovered,
            retired,
        })
    }
}
