//! Rebuildable SQLite vector cache with model-version and source-version boundaries.
use crate::provider::{Provider, validate_vectors};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub offset: usize,
    pub text: String,
    pub distance: f64,
}
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub updated: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub chunks: usize,
}
pub struct Index {
    conn: Connection,
}
fn vec_init() -> Result<()> {
    static INIT: OnceLock<i32> = OnceLock::new();
    let code = *INIT.get_or_init(|| {
        // SAFETY: sqlite-vec exports the SQLite extension entrypoint. This is the upstream
        // registration ABI, using the same bundled SQLite library as every rusqlite consumer.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut std::ffi::c_char,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> std::ffi::c_int,
            >(
                sqlite_vec::sqlite3_vec_init as *const ()
            )))
        }
    });
    if code != rusqlite::ffi::SQLITE_OK {
        bail!("Register vector extension: {code}");
    }
    Ok(())
}
fn blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|n| n.to_le_bytes()).collect()
}
fn normalize(vector: &[f32]) -> Result<Vec<f32>> {
    validate_vectors(&[vector.to_vec()], 1)?;
    let norm = vector
        .iter()
        .map(|n| (*n as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    Ok(vector.iter().map(|n| (*n as f64 / norm) as f32).collect())
}
fn chunks(source: &str) -> Vec<(usize, String)> {
    let base = notes_markdown::frontmatter(source)
        .range
        .map_or(0, |r| r.end);
    let mut output = Vec::new();
    let mut start = base;
    while start < source.len() {
        let mut end = (start + 2048).min(source.len());
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        if end < source.len()
            && let Some(newline) = source[start..end].rfind('\n').filter(|n| *n > 512)
        {
            end = start + newline + 1;
        }
        if !source[start..end].trim().is_empty() {
            output.push((start, source[start..end].to_owned()));
        }
        if end == source.len() {
            break;
        }
        let mut next = end.saturating_sub(256).max(start + 1);
        while !source.is_char_boundary(next) {
            next += 1;
        }
        start = next;
    }
    output
}
impl Index {
    pub fn open(path: &Path) -> Result<Self> {
        vec_init()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS semantic_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS semantic_docs(path TEXT PRIMARY KEY,fingerprint TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS semantic_chunks(id INTEGER PRIMARY KEY,path TEXT NOT NULL,offset INTEGER NOT NULL,text TEXT NOT NULL);CREATE INDEX IF NOT EXISTS semantic_paths ON semantic_chunks(path);")?;
        Ok(Self { conn })
    }
    fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM semantic_meta WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }
    fn prepare(&mut self, profile: &str) -> Result<()> {
        if self.meta("profile")?.as_deref() != Some(profile) {
            let tx = self.conn.transaction()?;
            tx.execute_batch("DROP TABLE IF EXISTS semantic_vectors;DELETE FROM semantic_chunks;DELETE FROM semantic_docs;DELETE FROM semantic_meta;")?;
            tx.execute("INSERT INTO semantic_meta VALUES('profile',?1)", [profile])?;
            tx.commit()?;
        }
        Ok(())
    }
    pub fn reconcile(
        &mut self,
        provider: &dyn Provider,
        root: &Path,
        paths: &[PathBuf],
        cancel: &AtomicBool,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<Stats> {
        let caps = provider.capabilities()?;
        if !caps.local || !caps.embedding || caps.embedding_profile.is_empty() {
            bail!("A configured local embedding provider is required");
        }
        self.prepare(&caps.embedding_profile)?;
        let mut stats = Stats::default();
        let mut seen = HashSet::new();
        for (index, path) in paths.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                bail!("Semantic indexing cancelled; completed documents remain available");
            }
            progress(index, paths.len());
            let key = path.to_string_lossy().to_string();
            let mut provider_failure = false;
            let result = (|| -> Result<bool> {
                use std::os::unix::fs::MetadataExt;
                let m = std::fs::symlink_metadata(path)?;
                if m.file_type().is_symlink()
                    || !path.canonicalize()?.starts_with(root)
                    || path.components().any(|c| c.as_os_str() == ".obsidian")
                {
                    bail!("Unsupported path");
                }
                seen.insert(key.clone());
                let fingerprint = format!(
                    "{}:{}:{}:{}:{}:{}",
                    m.ino(),
                    m.len(),
                    m.mtime(),
                    m.mtime_nsec(),
                    m.ctime(),
                    m.ctime_nsec()
                );
                let old: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT fingerprint FROM semantic_docs WHERE path=?1",
                        [&key],
                        |r| r.get(0),
                    )
                    .optional()?;
                if old.as_deref() == Some(&fingerprint) {
                    return Ok(false);
                }
                let doc = notes_vault::load(path)?;
                let chunks = chunks(&doc.text);
                let mut vectors = Vec::new();
                for batch in chunks.chunks(16) {
                    if cancel.load(Ordering::Relaxed) {
                        bail!("Semantic indexing cancelled");
                    }
                    let input = batch
                        .iter()
                        .map(|(_, text)| text.clone())
                        .collect::<Vec<_>>();
                    let response = match provider.embed(&input) {
                        Ok(response) => response,
                        Err(error) => {
                            provider_failure = true;
                            return Err(error);
                        }
                    };
                    if response.profile != caps.embedding_profile {
                        bail!("Embedding model changed while indexing; restart the rebuild");
                    }
                    validate_vectors(&response.vectors, input.len())
                        .inspect_err(|_| provider_failure = true)?;
                    for vector in response.vectors {
                        vectors.push(normalize(&vector)?);
                    }
                }
                if let Some(dim) = vectors.first().map(Vec::len) {
                    if let Some(previous) = self.meta("dimensions")? {
                        if previous.parse::<usize>()? != dim {
                            bail!("Embedding dimensions changed without a model profile change");
                        }
                    } else {
                        self.conn.execute_batch(&format!("CREATE VIRTUAL TABLE semantic_vectors USING vec0(embedding float[{dim}]);"))?;
                        self.conn.execute(
                            "INSERT INTO semantic_meta VALUES('dimensions',?1)",
                            [dim.to_string()],
                        )?;
                    }
                }
                let tx = self.conn.transaction()?;
                if !vectors.is_empty()
                    || tx.query_row(
                        "SELECT count(*) FROM sqlite_master WHERE name='semantic_vectors'",
                        [],
                        |r| r.get::<_, i64>(0),
                    )? > 0
                {
                    tx.execute("DELETE FROM semantic_vectors WHERE rowid IN(SELECT id FROM semantic_chunks WHERE path=?1)",[&key])?;
                }
                tx.execute("DELETE FROM semantic_chunks WHERE path=?1", [&key])?;
                for ((offset, text), vector) in chunks.iter().zip(vectors.iter()) {
                    tx.execute(
                        "INSERT INTO semantic_chunks(path,offset,text) VALUES(?1,?2,?3)",
                        params![key, *offset as i64, text],
                    )?;
                    let id = tx.last_insert_rowid();
                    tx.execute(
                        "INSERT INTO semantic_vectors(rowid,embedding) VALUES(?1,?2)",
                        params![id, blob(vector)],
                    )?;
                }
                tx.execute(
                    "INSERT OR REPLACE INTO semantic_docs VALUES(?1,?2)",
                    params![key, fingerprint],
                )?;
                tx.commit()?;
                stats.chunks += chunks.len();
                Ok(true)
            })();
            match result {
                Ok(true) => stats.updated += 1,
                Ok(false) => stats.unchanged += 1,
                Err(e) => {
                    if e.downcast_ref::<rusqlite::Error>().is_some()
                        || provider_failure
                        || cancel.load(Ordering::Relaxed)
                        || e.to_string().contains("Embedding")
                        || e.to_string().contains("Assistant")
                    {
                        return Err(e);
                    }
                    seen.remove(&key);
                    stats.skipped += 1;
                }
            }
        }
        let previous = self
            .conn
            .prepare("SELECT path FROM semantic_docs")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let has_vectors = self.meta("dimensions")?.is_some();
        let tx = self.conn.transaction()?;
        for path in previous {
            if !seen.contains(&path) {
                if has_vectors {
                    tx.execute("DELETE FROM semantic_vectors WHERE rowid IN(SELECT id FROM semantic_chunks WHERE path=?1)",[&path])?;
                }
                tx.execute("DELETE FROM semantic_chunks WHERE path=?1", [&path])?;
                tx.execute("DELETE FROM semantic_docs WHERE path=?1", [&path])?;
            }
        }
        tx.commit()?;
        progress(paths.len(), paths.len());
        Ok(stats)
    }
    pub fn search(&self, provider: &dyn Provider, query: &str, limit: usize) -> Result<Vec<Hit>> {
        if self.meta("dimensions")?.is_none() {
            return Ok(Vec::new());
        }
        let result = provider.embed(&[query.chars().take(8192).collect()])?;
        if self.meta("profile")?.as_deref() != Some(&result.profile) {
            bail!("Embedding model changed; rebuild the semantic index");
        }
        validate_vectors(&result.vectors, 1)?;
        let vector = normalize(&result.vectors[0])?;
        self.nearest(&vector, limit)
    }
    fn nearest(&self, vector: &[f32], limit: usize) -> Result<Vec<Hit>> {
        let dim = self
            .meta("dimensions")?
            .context("Semantic index is empty")?
            .parse::<usize>()?;
        if vector.len() != dim {
            bail!("Embedding dimensions do not match the semantic index");
        }
        let count = (limit.clamp(1, 100) * 4).min(400);
        let rows=self.conn.prepare("SELECT rowid,distance FROM semantic_vectors WHERE embedding MATCH ?1 AND k=?2 ORDER BY distance")?.query_map(params![blob(vector),count as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,f64>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        for (id, distance) in rows {
            let hit = self.conn.query_row(
                "SELECT path,offset,text FROM semantic_chunks WHERE id=?1",
                [id],
                |r| {
                    Ok(Hit {
                        path: PathBuf::from(r.get::<_, String>(0)?),
                        offset: r.get::<_, i64>(1)? as usize,
                        text: r.get(2)?,
                        distance,
                    })
                },
            )?;
            if seen.insert(hit.path.clone()) {
                hits.push(hit);
                if hits.len() >= limit {
                    break;
                }
            }
        }
        Ok(hits)
    }
    pub fn related(&self, path: &Path, limit: usize) -> Result<Vec<Hit>> {
        if self.meta("dimensions")?.is_none() {
            return Ok(Vec::new());
        }
        let bytes=self.conn.query_row("SELECT embedding FROM semantic_vectors WHERE rowid=(SELECT id FROM semantic_chunks WHERE path=?1 ORDER BY offset LIMIT 1)",[path.to_string_lossy().as_ref()],|r|r.get::<_,Vec<u8>>(0)).optional()?;
        let Some(bytes) = bytes else {
            return Ok(Vec::new());
        };
        let vector = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect::<Vec<_>>();
        Ok(self
            .nearest(&vector, limit + 1)?
            .into_iter()
            .filter(|h| h.path != path)
            .take(limit)
            .collect())
    }
    pub fn clear(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DROP TABLE IF EXISTS semantic_vectors;DELETE FROM semantic_docs;DELETE FROM semantic_chunks;DELETE FROM semantic_meta;")?;
        tx.commit()?;
        Ok(())
    }
    pub fn hybrid(
        &self,
        provider: &dyn Provider,
        keyword_db: &Path,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        let semantic = self.search(provider, query, limit.max(30))?;
        let keyword = notes_vault::search(keyword_db, query).unwrap_or_default();
        let mut ranks = std::collections::HashMap::<PathBuf, (Hit, f64)>::new();
        for (rank, hit) in semantic.into_iter().enumerate() {
            let path = hit.path.clone();
            let entry = ranks.entry(path).or_insert((hit, 0.0));
            entry.1 += 1.0 / (60.0 + rank as f64);
        }
        for (rank, hit) in keyword.into_iter().enumerate() {
            let entry = ranks.entry(hit.path.clone()).or_insert((
                Hit {
                    path: hit.path,
                    offset: hit.offset.unwrap_or(0),
                    text: hit.snippet,
                    distance: f64::NAN,
                },
                0.0,
            ));
            entry.1 += 1.0 / (60.0 + rank as f64);
        }
        let mut all = ranks.into_values().collect::<Vec<_>>();
        all.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.path.cmp(&b.0.path)));
        Ok(all.into_iter().take(limit).map(|(hit, _)| hit).collect())
    }
    pub fn document_count(&self) -> Result<usize> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM semantic_docs", [], |r| {
                r.get::<_, i64>(0)
            })? as usize)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Capabilities, Embeddings};
    struct Mock {
        calls: std::sync::atomic::AtomicUsize,
    }
    impl Provider for Mock {
        fn capabilities(&self) -> Result<Capabilities> {
            Ok(Capabilities {
                api_version: 1,
                generation: true,
                local: true,
                embedding: true,
                embedding_model: "fixture".into(),
                embedding_profile: "fixture-v1".into(),
                embedding_endpoint: String::new(),
            })
        }
        fn generate(&self, _: &str, _: &str) -> Result<String> {
            Ok(String::new())
        }
        fn embed(&self, input: &[String]) -> Result<Embeddings> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(Embeddings {
                profile: "fixture-v1".into(),
                vectors: input
                    .iter()
                    .map(|text| {
                        if text.to_lowercase().contains("rust") {
                            vec![1.0, 0.1]
                        } else {
                            vec![0.1, 1.0]
                        }
                    })
                    .collect(),
            })
        }
    }
    #[test]
    fn semantic_cache_is_incremental_and_rebuildable_without_models() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("vault");
        std::fs::create_dir(&root).unwrap();
        let a = root.join("Rust.md");
        let b = root.join("Garden.md");
        std::fs::write(&a, "---\ntitle: Keep\n---\n# Rust\nCafé 🦀 local writing").unwrap();
        std::fs::write(&b, "# Garden\nPlants and flowers").unwrap();
        let provider = Mock {
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let mut index = Index::open(&t.path().join("vectors.sqlite")).unwrap();
        let cancel = AtomicBool::new(false);
        let paths = vec![a.clone(), b.clone()];
        assert_eq!(
            index
                .reconcile(&provider, &root, &paths, &cancel, |_, _| {})
                .unwrap()
                .updated,
            2
        );
        let calls = provider.calls.load(Ordering::Relaxed);
        assert_eq!(
            index
                .reconcile(&provider, &root, &paths, &cancel, |_, _| {})
                .unwrap()
                .unchanged,
            2
        );
        assert_eq!(provider.calls.load(Ordering::Relaxed), calls);
        let hits = index.search(&provider, "Rust", 10).unwrap();
        assert_eq!(hits[0].path, a);
        assert!(hits[0].offset > 0);
        assert_eq!(index.related(&a, 10).unwrap()[0].path, b);
        assert_eq!(index.document_count().unwrap(), 2);
        std::fs::remove_file(&b).unwrap();
        index
            .reconcile(
                &provider,
                &root,
                std::slice::from_ref(&a),
                &cancel,
                |_, _| {},
            )
            .unwrap();
        assert_eq!(index.document_count().unwrap(), 1);
        index.clear().unwrap();
        assert_eq!(index.document_count().unwrap(), 0);
        assert!(index.search(&provider, "Rust", 10).unwrap().is_empty());
        assert!(std::fs::read_to_string(a).unwrap().contains("title: Keep"));
        let long = "# 🦀 café\n".repeat(1000);
        for (offset, part) in chunks(&long) {
            assert!(long.is_char_boundary(offset));
            assert!(part.len() <= 2048);
        }
    }
}
