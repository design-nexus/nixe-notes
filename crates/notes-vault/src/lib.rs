//! Blocking filesystem APIs: callers must run these on an I/O worker.
pub mod canvas;
pub mod history;
pub mod integrations;
pub mod knowledge;
mod links;
mod search;
pub mod workspace;
use anyhow::{Context, Result, bail};
use gio::prelude::*;
pub use links::local_target as local_link_target;
pub use links::{Backlink, ResolvedLink, backlinks, resolve_link};
pub use search::{SearchHit, mentions, rebuild_index, reconcile_index, search, update_index};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Version {
    pub etag: String,
    pub digest: blake3::Hash,
}
#[derive(Clone, Debug)]
pub struct Loaded {
    pub path: PathBuf,
    pub text: String,
    pub bom: bool,
    pub version: Version,
    pub read_only: bool,
}
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub directory: bool,
    pub symlink: bool,
}
#[derive(Debug)]
pub enum SaveError {
    Conflict,
    Failure(anyhow::Error),
}
impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict => write!(
                f,
                "The file changed or was removed outside Notes. Your edits are retained."
            ),
            Self::Failure(e) => write!(f, "{e:#}"),
        }
    }
}

fn protected(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == ".obsidian")
        || path
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .is_some_and(|p| p.components().any(|c| c.as_os_str() == ".obsidian"))
}
pub fn vault_root(path: &Path) -> Result<PathBuf> {
    if protected(path) {
        bail!("Obsidian configuration is protected");
    }
    let root = path.canonicalize()?;
    if !root.is_dir() {
        bail!("Choose a directory for the vault");
    }
    Ok(root)
}

pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("markdown"))
}

pub fn children(root: &Path, dir: &Path) -> Result<Vec<Entry>> {
    Ok(children_all(root, dir)?
        .into_iter()
        .filter(|e| e.directory || is_markdown(&e.path))
        .collect())
}
pub fn children_all(root: &Path, dir: &Path) -> Result<Vec<Entry>> {
    let dir = dir.canonicalize()?;
    if protected(&dir) {
        bail!("Obsidian configuration is protected");
    }
    if !dir.starts_with(root) {
        bail!("Folder is outside the vault");
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type()?;
        let directory = kind.is_dir();
        if directory || kind.is_file() || kind.is_symlink() {
            entries.push(Entry {
                path: entry.path(),
                directory,
                symlink: kind.is_symlink(),
            });
        }
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.path.file_name().cmp(&b.path.file_name()))
    });
    Ok(entries)
}

pub fn load(path: &Path) -> Result<Loaded> {
    let symlink = std::fs::symlink_metadata(path)?.file_type().is_symlink();
    let (bytes, etag) = gio::File::for_path(path).load_contents(gio::Cancellable::NONE)?;
    let digest = blake3::hash(&bytes);
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let text = std::str::from_utf8(if bom { &bytes[3..] } else { &bytes })
        .context("This file is not UTF-8; it has not been changed")?
        .to_owned();
    Ok(Loaded {
        path: path.to_owned(),
        text,
        bom,
        version: Version {
            etag: etag
                .context("Filesystem does not provide safe save versioning")?
                .to_string(),
            digest,
        },
        read_only: symlink || protected(path) || std::fs::metadata(path)?.permissions().readonly(),
    })
}

pub fn encode(text: &str, bom: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 3);
    if bom {
        bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

pub fn save(path: &Path, text: &str, bom: bool, expected: &Version) -> Result<Version, SaveError> {
    let operation = || -> Result<Version, SaveError> {
        if protected(path) {
            return Err(SaveError::Failure(anyhow::anyhow!(
                "Obsidian configuration is protected"
            )));
        }
        let meta = std::fs::symlink_metadata(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SaveError::Conflict
            } else {
                SaveError::Failure(e.into())
            }
        })?;
        if meta.file_type().is_symlink() {
            return Err(SaveError::Failure(anyhow::anyhow!(
                "Symbolic links are read-only in Notes"
            )));
        }
        let (current, etag) = gio::File::for_path(path)
            .load_contents(gio::Cancellable::NONE)
            .map_err(|e| SaveError::Failure(e.into()))?;
        if blake3::hash(&current) != expected.digest || etag.as_deref() != Some(&expected.etag) {
            return Err(SaveError::Conflict);
        }
        let bytes = encode(text, bom);
        let etag = gio::File::for_path(path)
            .replace_contents(
                &bytes,
                Some(&expected.etag),
                false,
                gio::FileCreateFlags::NONE,
                gio::Cancellable::NONE,
            )
            .map_err(|e| {
                if e.matches(gio::IOErrorEnum::WrongEtag) {
                    SaveError::Conflict
                } else {
                    SaveError::Failure(e.into())
                }
            })?
            .ok_or_else(|| SaveError::Failure(anyhow::anyhow!("No version returned after save")))?;
        Ok(Version {
            etag: etag.to_string(),
            digest: blake3::hash(&bytes),
        })
    };
    operation()
}

/// Write a new copy atomically and refuse to replace an existing file.
pub fn save_copy(path: &Path, text: &str, bom: bool) -> Result<()> {
    if protected(path) {
        bail!("Obsidian configuration is protected");
    }
    let parent = path.parent().context("Copy needs a destination folder")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&encode(text, bom))?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path).map_err(|e| e.error)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// Create without replacing any existing entry. Names must be one path component.
pub fn create(root: &Path, dir: &Path, name: &str, directory: bool) -> Result<PathBuf> {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        bail!("Use a visible name without path separators");
    }
    let dir = dir.canonicalize()?;
    if protected(&dir) {
        bail!("Obsidian configuration is protected");
    }
    if !dir.starts_with(root) {
        bail!("Destination is outside the vault");
    }
    let name = if !directory && !is_markdown(Path::new(name)) {
        format!("{name}.md")
    } else {
        name.to_owned()
    };
    let path = dir.join(name);
    if directory {
        std::fs::create_dir(&path)?;
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?
            .sync_all()?;
    }
    Ok(path)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Draft {
    pub path: PathBuf,
    pub text: String,
    pub bom: bool,
    pub base_etag: String,
    pub base_digest: String,
}
pub fn draft_path(state: &Path, path: &Path) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    state.join("recovery").join(format!(
        "{}.json",
        blake3::hash(path.as_os_str().as_bytes()).to_hex()
    ))
}
pub fn write_draft(state: &Path, draft: &Draft) -> Result<()> {
    let target = draft_path(state, &draft.path);
    let dir = target.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut file, draft)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(&target).map_err(|e| e.error)?;
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}
pub fn read_draft(state: &Path, path: &Path) -> Result<Option<Draft>> {
    let file = draft_path(state, path);
    match std::fs::read(file) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn remove_draft(state: &Path, path: &Path) -> Result<()> {
    match std::fs::remove_file(draft_path(state, path)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
/// Recover deleted notes into an editable draft without recreating their disk file.
pub fn open_with_recovery(state: &Path, path: &Path) -> Result<(Loaded, Option<Draft>)> {
    let draft = read_draft(state, path)?;
    match load(path) {
        Ok(doc) => Ok((doc, draft)),
        Err(error) => {
            if std::fs::symlink_metadata(path)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                && let Some(draft) = draft
            {
                let doc = Loaded {
                    path: path.to_owned(),
                    text: String::new(),
                    bom: draft.bom,
                    version: Version {
                        etag: draft.base_etag.clone(),
                        digest: blake3::Hash::from_hex(&draft.base_digest)?,
                    },
                    read_only: false,
                };
                return Ok((doc, Some(draft)));
            }
            Err(error)
        }
    }
}
pub fn recovery_paths(state: &Path) -> Result<Vec<PathBuf>> {
    let dir = state.join("recovery");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|s| s == "json") {
            let draft: Draft = serde_json::from_slice(&std::fs::read(entry.path())?)?;
            result.push(draft.path);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_bytes_and_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Café 🦀.md");
        let bytes = include_bytes!("../../../tests/fixtures/obsidian/fidelity.md");
        std::fs::write(&path, bytes).unwrap();
        let doc = load(&path).unwrap();
        let version = save(&path, &doc.text, doc.bom, &doc.version).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::write(&path, b"external").unwrap();
        assert!(matches!(
            save(&path, "my changes", false, &version),
            Err(SaveError::Conflict)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            save(&path, "my changes", false, &version),
            Err(SaveError::Conflict)
        ));
    }
    #[test]
    fn creation_recovery_and_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = create(&root, &root, "Hello", false).unwrap();
        assert!(create(&root, &root, "Hello", false).is_err());
        assert!(create(&root, &root, "../escape", false).is_err());
        let link = root.join("link.md");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let linked = load(&link).unwrap();
        assert!(linked.read_only);
        assert!(save(&link, "bad", false, &linked.version).is_err());
        std::fs::create_dir(root.join(".obsidian")).unwrap();
        assert!(
            !children(&root, &root)
                .unwrap()
                .iter()
                .any(|e| e.path.ends_with(".obsidian"))
        );
        let doc = load(&path).unwrap();
        let draft = Draft {
            path: path.clone(),
            text: "recover".into(),
            bom: false,
            base_etag: doc.version.etag,
            base_digest: doc.version.digest.to_hex().to_string(),
        };
        write_draft(&root, &draft).unwrap();
        assert_eq!(read_draft(&root, &path).unwrap().unwrap().text, "recover");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            open_with_recovery(&root, &path).unwrap().1.unwrap().text,
            "recover"
        );
        assert!(!path.exists());
        save_copy(&path, "copied", false).unwrap();
        assert!(save_copy(&path, "overwrite", false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "copied");
        remove_draft(&root, &path).unwrap();
        assert!(read_draft(&root, &path).unwrap().is_none());
    }
    #[test]
    fn rejects_invalid_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.md");
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(load(&path).is_err());
        assert_eq!(std::fs::read(path).unwrap(), [0xff, 0xfe]);
    }
}
