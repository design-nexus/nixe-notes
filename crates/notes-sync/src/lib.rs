//! Personal file synchronization. All APIs block: use a dedicated network worker.
mod webdav;
use anyhow::{Context, Result, bail, ensure};
use gio::prelude::*;
pub use nixe_sync::{Secret, delay, lookup_secret, store_secret};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
pub use webdav::WebDav;

pub const MAX_FILE: u64 = 128 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub root: PathBuf,
    /// Full URL of a dedicated WebDAV vault collection; never contains credentials.
    pub endpoint: String,
    pub username: String,
    pub enabled: bool,
    /// Relative folders/files excluded from both directions. Hidden paths are always excluded.
    pub excluded: Vec<String>,
}
impl Config {
    pub fn id(&self) -> String {
        blake3::hash(
            format!(
                "{}\n{}\n{}",
                self.root.display(),
                self.endpoint,
                self.username
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string()
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.root.is_absolute() && self.root.is_dir(),
            "Choose an existing vault"
        );
        ensure!(
            !self.username.trim().is_empty() && !self.username.chars().any(char::is_control),
            "Enter a WebDAV username"
        );
        webdav::endpoint(&self.endpoint, false)?;
        for p in &self.excluded {
            validate_path(p.trim_end_matches('/'))?;
        }
        Ok(())
    }
    pub fn includes(&self, path: &str) -> bool {
        validate_path(path).is_ok()
            && !self.excluded.iter().any(|e| {
                let e = e.trim_end_matches('/');
                path == e || path.strip_prefix(e).is_some_and(|p| p.starts_with('/'))
            })
    }
}
pub fn validate_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && path.len() <= 4096
            && !path.contains('\\')
            && !path.chars().any(char::is_control),
        "Invalid vault path"
    );
    ensure!(
        path.split('/')
            .all(|p| !p.is_empty() && !p.starts_with('.') && p != ".."),
        "Hidden, absolute, or traversing paths are excluded"
    );
    Ok(())
}
#[derive(Clone, Debug)]
pub struct RemoteFile {
    pub etag: String,
    pub size: u64,
}
pub trait Transport {
    fn list(&mut self, cancel: &AtomicBool) -> Result<BTreeMap<String, RemoteFile>>;
    fn get(&mut self, path: &str, etag: &str) -> Result<Vec<u8>>;
    /// None means create-only; Some means replace precisely this remote version.
    fn put(&mut self, path: &str, bytes: &[u8], etag: Option<&str>) -> Result<String>;
    fn delete(&mut self, path: &str, etag: &str) -> Result<()>;
}
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub uploaded: usize,
    pub downloaded: usize,
    pub conflicts: usize,
    pub deferred: usize,
}
impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} uploaded · {} downloaded · {} conflicts · {} deferred",
            self.uploaded, self.downloaded, self.conflicts, self.deferred
        )
    }
}
#[derive(Clone, Debug)]
pub struct Conflict {
    pub path: String,
    pub local: Option<Vec<u8>>,
    pub remote: Option<Vec<u8>>,
    pub etag: Option<String>,
}
#[derive(Clone, Copy, Debug)]
pub enum Resolution {
    Local,
    Remote,
    Both,
}
pub struct Engine {
    db: Connection,
    config: Config,
    state: PathBuf,
}
fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "Synchronization cancelled");
    Ok(())
}
impl Engine {
    pub fn open(state: &Path, config: Config) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let dir = state.join("sync");
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let path = dir.join(format!("{}.sqlite", config.id()));
        let db = Connection::open(&path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS baseline(path TEXT PRIMARY KEY,hash TEXT NOT NULL,etag TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS conflicts(path TEXT PRIMARY KEY,local BLOB,remote BLOB,etag TEXT);
            CREATE TABLE IF NOT EXISTS versions(id INTEGER PRIMARY KEY,path TEXT NOT NULL,content BLOB NOT NULL,created INTEGER NOT NULL DEFAULT(unixepoch()));")?;
        Ok(Self {
            db,
            config,
            state: state.into(),
        })
    }
    fn baseline(&self, path: &str) -> Result<Option<(String, String)>> {
        Ok(self
            .db
            .query_row(
                "SELECT hash,etag FROM baseline WHERE path=?1",
                [path],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
    fn commit(&self, path: &str, hash: &str, etag: &str) -> Result<()> {
        self.db.execute("INSERT INTO baseline VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET hash=excluded.hash,etag=excluded.etag",params![path,hash,etag])?;
        self.db
            .execute("DELETE FROM conflicts WHERE path=?1", [path])?;
        Ok(())
    }
    fn archive(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.db.execute(
            "INSERT INTO versions(path,content) VALUES(?1,?2)",
            params![path, bytes],
        )?;
        self.db.execute("DELETE FROM versions WHERE path=?1 AND id NOT IN(SELECT id FROM versions WHERE path=?1 ORDER BY id DESC LIMIT 50)",[path])?;
        Ok(())
    }
    pub fn conflict_paths(&self) -> Result<Vec<String>> {
        let paths = self
            .db
            .prepare("SELECT path FROM conflicts ORDER BY path")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(paths
            .into_iter()
            .filter(|p| self.config.includes(p))
            .collect())
    }
    pub fn conflict_for(&self, path: &str) -> Result<Conflict> {
        Ok(self.db.query_row(
            "SELECT path,local,remote,etag FROM conflicts WHERE path=?1",
            [path],
            |r| {
                Ok(Conflict {
                    path: r.get(0)?,
                    local: r.get(1)?,
                    remote: r.get(2)?,
                    etag: r.get(3)?,
                })
            },
        )?)
    }
    pub fn conflicts(&self) -> Result<Vec<Conflict>> {
        Ok(self
            .db
            .prepare("SELECT path,local,remote,etag FROM conflicts ORDER BY path")?
            .query_map([], |r| {
                Ok(Conflict {
                    path: r.get(0)?,
                    local: r.get(1)?,
                    remote: r.get(2)?,
                    etag: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?)
    }
    pub fn versions(&self, path: &str) -> Result<Vec<(i64, i64, usize)>> {
        Ok(self
            .db
            .prepare(
                "SELECT id,created,length(content) FROM versions WHERE path=?1 ORDER BY id DESC",
            )?
            .query_map([path], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as usize))
            })?
            .collect::<std::result::Result<_, _>>()?)
    }
    pub fn version(&self, id: i64) -> Result<Vec<u8>> {
        Ok(self
            .db
            .query_row("SELECT content FROM versions WHERE id=?1", [id], |r| {
                r.get(0)
            })?)
    }
    pub fn export_version(&self, id: i64, destination: &Path) -> Result<()> {
        let bytes = self.version(id)?;
        let mut f =
            tempfile::NamedTempFile::new_in(destination.parent().context("Choose a destination")?)?;
        f.write_all(&bytes)?;
        f.as_file().sync_all()?;
        f.persist_noclobber(destination).map_err(|e| e.error)?;
        Ok(())
    }
    fn conflict(
        &self,
        path: &str,
        local: Option<&[u8]>,
        remote: Option<&[u8]>,
        etag: Option<&str>,
    ) -> Result<()> {
        self.db.execute("INSERT INTO conflicts VALUES(?1,?2,?3,?4) ON CONFLICT(path) DO UPDATE SET local=excluded.local,remote=excluded.remote,etag=excluded.etag",params![path,local,remote,etag])?;
        Ok(())
    }
    pub fn run(
        &self,
        remote: &mut impl Transport,
        blocked: &BTreeSet<PathBuf>,
        cancel: &AtomicBool,
    ) -> Result<Report> {
        cancelled(cancel)?;
        let local = scan(&self.config)?;
        // A failed/incomplete remote listing must never be interpreted as deletion.
        let files = remote.list(cancel)?;
        let mut paths: BTreeSet<String> = local.keys().chain(files.keys()).cloned().collect();
        for p in self
            .db
            .prepare("SELECT path FROM baseline")?
            .query_map([], |r| r.get::<_, String>(0))?
        {
            paths.insert(p?);
        }
        let mut report = Report::default();
        for path in paths {
            cancelled(cancel)?;
            if !self.config.includes(&path) {
                continue;
            }
            let full = self.config.root.join(&path);
            if blocked.contains(&full) || notes_vault::draft_path(&self.state, &full).exists() {
                report.deferred += 1;
                continue;
            }
            let bytes = local
                .get(&path)
                .map(|_| read_local(&self.config.root, &path))
                .transpose()?
                .flatten();
            let hash = bytes.as_deref().map(digest);
            let base = self.baseline(&path)?;
            let file = files.get(&path);
            if let (Some(h), Some((b, e)), Some(r)) = (&hash, &base, file)
                && h == b
                && e == &r.etag
            {
                self.db
                    .execute("DELETE FROM conflicts WHERE path=?1", [&path])?;
                continue;
            }
            let other = file.map(|f| remote.get(&path, &f.etag)).transpose()?;
            let other_hash = other.as_deref().map(digest);
            if hash == other_hash {
                if let (Some(h), Some(r)) = (&hash, file) {
                    self.commit(&path, h, &r.etag)?;
                } else {
                    self.db
                        .execute("DELETE FROM baseline WHERE path=?1", [&path])?;
                    self.db
                        .execute("DELETE FROM conflicts WHERE path=?1", [&path])?;
                }
                continue;
            }
            // A first merge never overwrites a different existing file. Deletions always need review.
            let upload = bytes.is_some() && other.is_none() && base.is_none()
                || base
                    .as_ref()
                    .is_some_and(|(b, _)| other_hash.as_ref() == Some(b))
                    && bytes.is_some();
            let download = other.is_some() && bytes.is_none() && base.is_none()
                || base.as_ref().is_some_and(|(b, _)| hash.as_ref() == Some(b)) && other.is_some();
            if upload {
                let data = bytes.as_deref().unwrap();
                if let Some(old) = other.as_deref() {
                    self.archive(&path, old)?;
                }
                cancelled(cancel)?;
                let etag = remote.put(&path, data, file.map(|f| f.etag.as_str()))?;
                self.commit(&path, hash.as_deref().unwrap(), &etag)?;
                report.uploaded += 1;
            } else if download {
                cancelled(cancel)?;
                let data = other.as_deref().unwrap();
                self.replace_local(&path, bytes.as_deref(), Some(data))?;
                self.commit(&path, other_hash.as_deref().unwrap(), &file.unwrap().etag)?;
                report.downloaded += 1;
            } else {
                self.conflict(
                    &path,
                    bytes.as_deref(),
                    other.as_deref(),
                    file.map(|f| f.etag.as_str()),
                )?;
            }
        }
        report.conflicts = self.conflict_paths()?.len();
        Ok(report)
    }
    pub fn resolve(
        &self,
        remote: &mut impl Transport,
        conflict: &Conflict,
        resolution: Resolution,
    ) -> Result<()> {
        validate_path(&conflict.path)?;
        ensure!(
            !notes_vault::draft_path(&self.state, &self.config.root.join(&conflict.path)).exists(),
            "Resolve the retained editor draft first"
        );
        ensure!(
            self.config.includes(&conflict.path),
            "This path is excluded from synchronization"
        );
        let local = read_local(&self.config.root, &conflict.path)?;
        ensure!(
            local == conflict.local,
            "Local version changed; synchronize again before resolving"
        );
        // Fetch against the reviewed ETag; a remote create/delete is also detected by conditional writes.
        if let Some(etag) = &conflict.etag {
            ensure!(
                Some(remote.get(&conflict.path, etag)?) == conflict.remote,
                "Remote content changed; synchronize again"
            );
        }
        match resolution {
            Resolution::Remote => {
                if conflict.remote.is_none() {
                    ensure!(
                        !remote
                            .list(&AtomicBool::new(false))?
                            .contains_key(&conflict.path),
                        "Remote version changed"
                    );
                }
                self.replace_local(&conflict.path, local.as_deref(), conflict.remote.as_deref())?;
                if let Some(bytes) = &conflict.remote {
                    self.commit(
                        &conflict.path,
                        &digest(bytes),
                        conflict.etag.as_deref().context("Missing remote version")?,
                    )?;
                } else {
                    self.forget(&conflict.path)?;
                }
            }
            Resolution::Local | Resolution::Both => {
                if matches!(resolution, Resolution::Both) {
                    ensure!(
                        local.is_some() && conflict.remote.is_some(),
                        "Keep both requires two existing versions"
                    );
                    let data = conflict.remote.as_deref().unwrap();
                    let p = Path::new(&conflict.path);
                    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("Note");
                    let ext = p
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(|s| format!(".{s}"))
                        .unwrap_or_default();
                    let name = format!("{stem} (remote {}){ext}", &digest(data)[..10]);
                    let target = p.with_file_name(name).to_string_lossy().into_owned();
                    match read_local(&self.config.root, &target)? {
                        Some(existing) => {
                            ensure!(existing == data, "Conflict-copy name is already in use")
                        }
                        None => self.replace_local(&target, None, Some(data))?,
                    }
                }
                if let Some(bytes) = &local {
                    if let Some(old) = &conflict.remote {
                        self.archive(&conflict.path, old)?;
                    }
                    let etag = remote.put(&conflict.path, bytes, conflict.etag.as_deref())?;
                    self.commit(&conflict.path, &digest(bytes), &etag)?;
                } else {
                    if let Some(etag) = &conflict.etag {
                        if let Some(old) = &conflict.remote {
                            self.archive(&conflict.path, old)?;
                        }
                        remote.delete(&conflict.path, etag)?;
                    }
                    self.forget(&conflict.path)?;
                }
            }
        }
        Ok(())
    }
    fn forget(&self, path: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM baseline WHERE path=?1", [path])?;
        self.db
            .execute("DELETE FROM conflicts WHERE path=?1", [path])?;
        Ok(())
    }
    fn replace_local(&self, path: &str, expected: Option<&[u8]>, new: Option<&[u8]>) -> Result<()> {
        let full = safe_path(&self.config.root, path, true)?;
        let current = read_local(&self.config.root, path)?;
        ensure!(
            current.as_deref() == expected,
            "Local file changed during synchronization"
        );
        if let Some(old) = expected {
            self.archive(path, old)?;
            if notes_vault::is_markdown(&full)
                && let Ok(text) = std::str::from_utf8(old)
            {
                notes_vault::history::capture(&self.state, &full, text, false, 200, 90)?;
            }
        }
        match (expected, new) {
            (None, Some(bytes)) => {
                let mut f = tempfile::NamedTempFile::new_in(full.parent().unwrap())?;
                f.write_all(bytes)?;
                f.as_file().sync_all()?;
                f.persist_noclobber(&full).map_err(|e| e.error)?;
            }
            (Some(old), Some(bytes)) => {
                let file = gio::File::for_path(&full);
                let (current, etag) = file.load_contents(gio::Cancellable::NONE)?;
                ensure!(&*current == old, "Local file changed during download");
                let etag = etag.context("Filesystem cannot provide safe versioning")?;
                file.replace_contents(
                    bytes,
                    Some(&etag),
                    false,
                    gio::FileCreateFlags::NONE,
                    gio::Cancellable::NONE,
                )?;
            }
            (Some(old), None) => {
                // Move, inspect, then discard the archived version. Never unlink a newly-created replacement.
                let holder = tempfile::Builder::new()
                    .prefix(".nixe-sync-")
                    .tempdir_in(full.parent().unwrap())?;
                let retained = holder.path().join("retained");
                rustix::fs::renameat_with(
                    rustix::fs::CWD,
                    &full,
                    rustix::fs::CWD,
                    &retained,
                    rustix::fs::RenameFlags::NOREPLACE,
                )?;
                let location = holder.keep();
                let moved = std::fs::read(&retained).with_context(|| {
                    format!("Deletion paused; retained file: {}", retained.display())
                })?;
                if moved != old {
                    let restored = rustix::fs::renameat_with(
                        rustix::fs::CWD,
                        &retained,
                        rustix::fs::CWD,
                        &full,
                        rustix::fs::RenameFlags::NOREPLACE,
                    );
                    if restored.is_err() {
                        bail!("Concurrent replacement retained at {}", location.display());
                    }
                    let _ = std::fs::remove_dir(&location);
                    bail!("Local file changed; deletion cancelled");
                }
                std::fs::remove_file(&retained)?;
                std::fs::remove_dir(&location)?;
            }
            (None, None) => {}
        }
        std::fs::File::open(full.parent().unwrap())?.sync_all()?;
        Ok(())
    }
}
fn safe_path(root: &Path, path: &str, create: bool) -> Result<PathBuf> {
    validate_path(path)?;
    let canonical = root.canonicalize()?;
    ensure!(canonical == root, "Vault location changed");
    let mut full = root.to_path_buf();
    let parts = path.split('/').collect::<Vec<_>>();
    for (i, p) in parts.iter().enumerate() {
        full.push(p);
        match std::fs::symlink_metadata(&full) {
            Ok(m) => {
                ensure!(!m.file_type().is_symlink(), "Symlink paths are excluded");
                if i + 1 < parts.len() {
                    ensure!(m.is_dir(), "Parent is not a folder");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if i + 1 < parts.len() && create {
                    std::fs::create_dir(&full)?;
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(full)
}
fn read_local(root: &Path, path: &str) -> Result<Option<Vec<u8>>> {
    let full = safe_path(root, path, false)?;
    match std::fs::File::open(full) {
        Ok(f) => {
            ensure!(f.metadata()?.is_file(), "Sync path is not a regular file");
            let mut data = Vec::new();
            f.take(MAX_FILE + 1).read_to_end(&mut data)?;
            ensure!(
                data.len() as u64 <= MAX_FILE,
                "File exceeds the 128 MiB transfer limit"
            );
            Ok(Some(data))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn scan(config: &Config) -> Result<BTreeMap<String, ()>> {
    fn walk(c: &Config, dir: &Path, out: &mut BTreeMap<String, ()>, depth: usize) -> Result<()> {
        ensure!(depth <= 64, "Vault nesting exceeds 64 folders");
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let p = e.path();
            let relative = p
                .strip_prefix(&c.root)?
                .to_str()
                .context("Sync needs UTF-8 filenames")?
                .to_owned();
            if !c.includes(&relative) {
                continue;
            }
            let kind = e.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                walk(c, &p, out, depth + 1)?;
            } else if kind.is_file() {
                ensure!(out.len() < 100_000, "Sync file limit exceeded");
                out.insert(relative, ());
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(config, &config.root, &mut out, 0)?;
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct MemoryRemote {
        files: BTreeMap<String, (Vec<u8>, u64)>,
        fail: bool,
        change_on_put: bool,
    }
    impl MemoryRemote {
        fn write(&mut self, p: &str, data: &[u8]) {
            let n = self.files.get(p).map_or(1, |(_, n)| n + 1);
            self.files.insert(p.into(), (data.into(), n));
        }
    }
    impl Transport for MemoryRemote {
        fn list(&mut self, _: &AtomicBool) -> Result<BTreeMap<String, RemoteFile>> {
            ensure!(!self.fail, "Offline");
            Ok(self
                .files
                .iter()
                .map(|(p, (b, n))| {
                    (
                        p.clone(),
                        RemoteFile {
                            etag: format!("\"{n}\""),
                            size: b.len() as u64,
                        },
                    )
                })
                .collect())
        }
        fn get(&mut self, p: &str, e: &str) -> Result<Vec<u8>> {
            let (b, n) = self.files.get(p).context("Removed")?;
            ensure!(e == format!("\"{n}\""), "412");
            Ok(b.clone())
        }
        fn put(&mut self, p: &str, b: &[u8], e: Option<&str>) -> Result<String> {
            if self.change_on_put {
                self.write(p, b"concurrent");
                self.change_on_put = false;
            }
            ensure!(
                e.map(str::to_owned) == self.files.get(p).map(|(_, n)| format!("\"{n}\"")),
                "412"
            );
            self.write(p, b);
            Ok(format!("\"{}\"", self.files[p].1))
        }
        fn delete(&mut self, p: &str, e: &str) -> Result<()> {
            self.get(p, e)?;
            self.files.remove(p);
            Ok(())
        }
    }
    #[test]
    fn divergent_edits_deletions_offline_and_interrupted_commits() {
        let vault = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let root = vault.path().canonicalize().unwrap();
        let config = Config {
            root: root.clone(),
            excluded: vec!["Private".into()],
            ..Default::default()
        };
        let engine = Engine::open(state.path(), config).unwrap();
        let mut remote = MemoryRemote::default();
        let cancel = AtomicBool::new(false);
        let blocked = BTreeSet::new();
        std::fs::write(
            root.join("Café.md"),
            b"\xef\xbb\xbf# Hello\r\n[[Untouched]]",
        )
        .unwrap();
        std::fs::create_dir(root.join(".obsidian")).unwrap();
        std::fs::write(root.join(".obsidian/config"), b"private").unwrap();
        std::fs::create_dir(root.join("Private")).unwrap();
        std::fs::write(root.join("Private/a.md"), b"private").unwrap();
        assert_eq!(
            engine.run(&mut remote, &blocked, &cancel).unwrap().uploaded,
            1
        );
        assert_eq!(remote.files.len(), 1);
        remote.write("Images/photo.bin", &[0, 255, 42]);
        assert_eq!(
            engine
                .run(&mut remote, &blocked, &cancel)
                .unwrap()
                .downloaded,
            1
        );
        assert_eq!(
            std::fs::read(root.join("Images/photo.bin")).unwrap(),
            [0, 255, 42]
        );
        std::fs::write(root.join("Café.md"), b"local").unwrap();
        remote.write("Café.md", b"remote");
        assert_eq!(
            engine
                .run(&mut remote, &blocked, &cancel)
                .unwrap()
                .conflicts,
            1
        );
        assert_eq!(std::fs::read(root.join("Café.md")).unwrap(), b"local");
        let conflict = engine.conflict_for("Café.md").unwrap();
        engine
            .resolve(&mut remote, &conflict, Resolution::Both)
            .unwrap();
        assert_eq!(remote.files["Café.md"].0, b"local");
        assert!(std::fs::read_dir(&root).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("(remote ")
        }));
        // A completed upload without a committed local baseline is recovered by content equality.
        std::fs::write(root.join("Café.md"), b"interrupted").unwrap();
        remote.write("Café.md", b"interrupted");
        engine.run(&mut remote, &blocked, &cancel).unwrap();
        assert_eq!(
            engine.baseline("Café.md").unwrap().unwrap().0,
            digest(b"interrupted")
        );
        std::fs::remove_file(root.join("Café.md")).unwrap();
        engine.run(&mut remote, &blocked, &cancel).unwrap();
        let deletion = engine.conflict_for("Café.md").unwrap();
        assert!(remote.files.contains_key("Café.md"));
        engine
            .resolve(&mut remote, &deletion, Resolution::Local)
            .unwrap();
        assert!(!remote.files.contains_key("Café.md"));
        remote.files.remove("Images/photo.bin");
        engine.run(&mut remote, &blocked, &cancel).unwrap();
        let deletion = engine.conflict_for("Images/photo.bin").unwrap();
        engine
            .resolve(&mut remote, &deletion, Resolution::Remote)
            .unwrap();
        assert!(!root.join("Images/photo.bin").exists());
        assert!(!engine.versions("Images/photo.bin").unwrap().is_empty());
        remote.fail = true;
        assert!(engine.run(&mut remote, &blocked, &cancel).is_err());
        assert_eq!(
            std::fs::read(root.join(".obsidian/config")).unwrap(),
            b"private"
        );
        remote.fail = false;
        std::fs::write(root.join("race.md"), b"base").unwrap();
        engine.run(&mut remote, &blocked, &cancel).unwrap();
        std::fs::write(root.join("race.md"), b"mine").unwrap();
        remote.change_on_put = true;
        assert!(engine.run(&mut remote, &blocked, &cancel).is_err());
        assert_eq!(remote.files["race.md"].0, b"concurrent");
        engine.run(&mut remote, &blocked, &cancel).unwrap();
        let stale = engine.conflict_for("race.md").unwrap();
        std::fs::write(root.join("race.md"), b"newer local").unwrap();
        assert!(
            engine
                .resolve(&mut remote, &stale, Resolution::Remote)
                .is_err()
        );
        assert_eq!(std::fs::read(root.join("race.md")).unwrap(), b"newer local");
        let external = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(external.path(), root.join("Link")).unwrap();
        remote.write("Link/escape.md", b"bad");
        assert!(engine.run(&mut remote, &blocked, &cancel).is_err());
        assert!(!external.path().join("escape.md").exists());
    }
}
