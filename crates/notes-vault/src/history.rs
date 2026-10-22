//! Durable, content-addressed local history; independent of the rebuildable index.
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use similar::{ChangeTag, TextDiff};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: i64,
    pub created: i64,
    pub bytes: usize,
}
#[derive(Clone, Debug)]
pub struct Revision {
    pub text: String,
    pub bom: bool,
}
#[derive(Clone, Debug)]
pub struct Patch {
    pub range: std::ops::Range<usize>,
    pub replacement: String,
}
fn connect(state: &Path) -> Result<Connection> {
    use std::os::unix::fs::PermissionsExt;
    let dir = state.join("history");
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let conn = Connection::open(dir.join("history.sqlite"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS objects(hash TEXT PRIMARY KEY, content BLOB NOT NULL);
        CREATE TABLE IF NOT EXISTS snapshots(id INTEGER PRIMARY KEY, path BLOB NOT NULL, created INTEGER NOT NULL, hash TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS history_path ON snapshots(path,id DESC);")?;
    Ok(conn)
}
fn key(path: &Path) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes()
}
pub fn capture(
    state: &Path,
    path: &Path,
    text: &str,
    bom: bool,
    keep: u32,
    days: u32,
) -> Result<()> {
    if keep == 0 {
        return Ok(());
    }
    let mut data = if bom {
        vec![0xef, 0xbb, 0xbf]
    } else {
        Vec::new()
    };
    data.extend_from_slice(text.as_bytes());
    let hash = blake3::hash(&data).to_hex().to_string();
    let mut conn = connect(state)?;
    let tx = conn.transaction()?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT hash FROM snapshots WHERE path=?1 ORDER BY id DESC LIMIT 1",
            [key(path)],
            |r| r.get(0),
        )
        .optional()?;
    if previous.as_deref() == Some(&hash) {
        return Ok(());
    }
    let created = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    tx.execute(
        "INSERT OR IGNORE INTO objects VALUES(?1,?2)",
        params![hash, data],
    )?;
    tx.execute(
        "INSERT INTO snapshots(path,created,hash) VALUES(?1,?2,?3)",
        params![key(path), created, hash],
    )?;
    tx.execute("DELETE FROM snapshots WHERE path=?1 AND id NOT IN (SELECT id FROM snapshots WHERE path=?1 ORDER BY id DESC LIMIT ?2)", params![key(path),keep])?;
    if days > 0 {
        tx.execute("DELETE FROM snapshots WHERE path=?1 AND created<?2 AND id!=(SELECT max(id) FROM snapshots WHERE path=?1)", params![key(path),created-days as i64*86400])?;
    }
    tx.execute(
        "DELETE FROM objects WHERE hash NOT IN (SELECT hash FROM snapshots)",
        [],
    )?;
    tx.commit()?;
    Ok(())
}
pub fn list(state: &Path, path: &Path) -> Result<Vec<Snapshot>> {
    let conn = connect(state)?;
    Ok(conn.prepare("SELECT s.id,s.created,length(o.content) FROM snapshots s JOIN objects o ON o.hash=s.hash WHERE path=?1 ORDER BY s.id DESC")?.query_map([key(path)], |r|Ok(Snapshot{id:r.get(0)?,created:r.get(1)?,bytes:r.get::<_,i64>(2)? as usize}))?.collect::<Result<Vec<_>,_>>()?)
}
pub fn read(state: &Path, path: &Path, id: i64) -> Result<Revision> {
    let conn = connect(state)?;
    let bytes: Vec<u8> = conn.query_row("SELECT content FROM objects JOIN snapshots USING(hash) WHERE snapshots.id=?1 AND path=?2",params![id,key(path)], |r|r.get(0)).context("Snapshot unavailable")?;
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    Ok(Revision {
        text: String::from_utf8(bytes[if bom { 3 } else { 0 }..].to_vec())?,
        bom,
    })
}
/// Follow vault moves without storing identifiers in the Markdown source.
pub fn relocate(state: &Path, source: &Path, destination: &Path) -> Result<()> {
    if !state.join("history/history.sqlite").exists() {
        return Ok(());
    }
    use std::os::unix::ffi::OsStringExt;
    let mut conn = connect(state)?;
    let tx = conn.transaction()?;
    let paths = tx
        .prepare("SELECT DISTINCT path FROM snapshots")?
        .query_map([], |r| r.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for bytes in paths {
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes));
        if let Ok(relative) = path.strip_prefix(source) {
            let new = if relative.as_os_str().is_empty() {
                destination.to_owned()
            } else {
                destination.join(relative)
            };
            tx.execute(
                "UPDATE snapshots SET path=?1 WHERE path=?2",
                params![key(&new), key(&path)],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}
pub fn compare(current: &str, historical: &str) -> (String, Vec<Patch>) {
    let diff = TextDiff::from_lines(current, historical);
    let mut display = String::new();
    let mut patches = Vec::new();
    let mut offset = 0;
    let mut patch: Option<Patch> = None;
    for change in diff.iter_all_changes() {
        let text = change.value().to_owned();
        display.push(match change.tag() {
            ChangeTag::Equal => ' ',
            ChangeTag::Delete => '-',
            ChangeTag::Insert => '+',
        });
        display.push_str(&text);
        match change.tag() {
            ChangeTag::Equal => {
                if let Some(p) = patch.take() {
                    patches.push(p);
                }
                offset += text.len();
            }
            ChangeTag::Delete => {
                let p = patch.get_or_insert_with(|| Patch {
                    range: offset..offset,
                    replacement: String::new(),
                });
                offset += text.len();
                p.range.end = offset;
            }
            ChangeTag::Insert => {
                patch
                    .get_or_insert_with(|| Patch {
                        range: offset..offset,
                        replacement: String::new(),
                    })
                    .replacement
                    .push_str(&text);
            }
        }
    }
    if let Some(p) = patch {
        patches.push(p);
    }
    (display, patches)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_deduplicate_prune_and_restore_exact_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let note = temp.path().join("Café.md");
        capture(temp.path(), &note, "one\r\n", true, 2, 30).unwrap();
        capture(temp.path(), &note, "one\r\n", true, 2, 30).unwrap();
        assert_eq!(list(temp.path(), &note).unwrap().len(), 1);
        let id = list(temp.path(), &note).unwrap()[0].id;
        let revision = read(temp.path(), &note, id).unwrap();
        assert!(revision.bom);
        assert_eq!(revision.text, "one\r\n");
        let current = "new\r\nunchanged\nend";
        let old = "old\r\nunchanged\nfinish";
        let (_, patches) = compare(current, old);
        let mut restored = current.to_owned();
        for p in patches.into_iter().rev() {
            restored.replace_range(p.range, &p.replacement);
        }
        assert_eq!(restored, old);
        capture(temp.path(), &note, "two", false, 2, 30).unwrap();
        capture(temp.path(), &note, "three", false, 2, 30).unwrap();
        assert_eq!(list(temp.path(), &note).unwrap().len(), 2);
        assert!(read(temp.path(), &note, id).is_err());
        let moved = temp.path().join("Renamed.md");
        relocate(temp.path(), &note, &moved).unwrap();
        assert_eq!(list(temp.path(), &moved).unwrap().len(), 2);
        assert!(list(temp.path(), &note).unwrap().is_empty());
    }
}
