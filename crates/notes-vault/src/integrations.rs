//! Public integration wire types and explicitly enabled launcher exports.
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherAction {
    pub api_version: u32,
    pub op: String,
    pub vault: PathBuf,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub append: bool,
}
impl LauncherAction {
    pub fn validate(&self) -> Result<()> {
        if self.api_version != 1
            || !self.vault.is_absolute()
            || self
                .vault
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            bail!("Invalid Notes launcher request");
        }
        match self.op.as_str() {
            "open" => {
                let p = self.path.as_ref().context("Missing note path")?;
                if !p.starts_with(&self.vault)
                    || !crate::is_markdown(p)
                    || p.components().any(|c| {
                        c.as_os_str() == ".obsidian" || matches!(c, std::path::Component::ParentDir)
                    })
                {
                    bail!("Choose a Markdown note inside the vault");
                }
            }
            "create" => {
                let title = self.title.as_deref().context("Missing note title")?;
                if title.is_empty()
                    || title.len() > 240
                    || title.starts_with('.')
                    || title.contains(['/', '\\', '\0', '\n', '\r'])
                {
                    bail!("Use a visible single-component note title");
                }
            }
            _ => bail!("Unsupported Notes launcher action"),
        };
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct LauncherHit {
    pub vault: PathBuf,
    pub path: PathBuf,
    pub title: String,
    pub snippet: String,
    pub offset: Option<usize>,
}
fn connect(db: &Path) -> Result<Connection> {
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = db.parent() {
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let conn = Connection::open(db)?;
    std::fs::set_permissions(db, std::fs::Permissions::from_mode(0o600))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL;
    CREATE TABLE IF NOT EXISTS launcher_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS launcher_vaults(path TEXT PRIMARY KEY,opened INTEGER NOT NULL);
    CREATE VIRTUAL TABLE IF NOT EXISTS launcher_notes USING fts5(path UNINDEXED,vault UNINDEXED,title,body,tags);
    CREATE TABLE IF NOT EXISTS launcher_stamps(path TEXT PRIMARY KEY,vault TEXT NOT NULL,fingerprint TEXT NOT NULL,fts_id INTEGER NOT NULL);CREATE INDEX IF NOT EXISTS launcher_vault ON launcher_stamps(vault);
    CREATE TABLE IF NOT EXISTS launcher_headings(path TEXT NOT NULL,title TEXT NOT NULL,offset INTEGER NOT NULL);CREATE INDEX IF NOT EXISTS launcher_heading_path ON launcher_headings(path);")?;
    Ok(conn)
}
pub fn export(db: &Path, root: &Path, target: &Path, enabled: bool) -> Result<usize> {
    let mut conn = connect(target)?;
    let tx = conn.transaction()?;
    if !enabled {
        tx.execute_batch("DELETE FROM launcher_notes;DELETE FROM launcher_stamps;DELETE FROM launcher_headings;DELETE FROM launcher_vaults;")?;
        tx.execute(
            "INSERT OR REPLACE INTO launcher_meta VALUES('enabled','false')",
            [],
        )?;
        tx.commit()?;
        return Ok(0);
    }
    tx.execute(
        "INSERT OR REPLACE INTO launcher_meta VALUES('enabled','true')",
        [],
    )?;
    let opened = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let vault = root.to_str().context("Launcher needs UTF-8 vault paths")?;
    tx.execute(
        "INSERT OR REPLACE INTO launcher_vaults VALUES(?1,?2)",
        params![vault, opened],
    )?;
    let mut source = crate::search::connect(db)?;
    let read = source.transaction()?;
    let stamps = read
        .prepare("SELECT path,fingerprint,fts_rowid FROM stamps")?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = std::collections::HashSet::new();
    let mut count = 0;
    for (path, fingerprint, rowid) in stamps {
        if !Path::new(&path).starts_with(root) {
            continue;
        }
        seen.insert(path.clone());
        let old: Option<String> = tx
            .query_row(
                "SELECT fingerprint FROM launcher_stamps WHERE path=?1",
                [&path],
                |r| r.get(0),
            )
            .optional()?;
        if old.as_deref() == Some(&fingerprint) {
            continue;
        }
        tx.execute("DELETE FROM launcher_notes WHERE rowid=(SELECT fts_id FROM launcher_stamps WHERE path=?1)",[&path])?;
        tx.execute("DELETE FROM launcher_headings WHERE path=?1", [&path])?;
        let (title, body, tags): (String, String, String) = read.query_row(
            "SELECT title,body,tags FROM notes WHERE rowid=?1",
            [rowid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        tx.execute(
            "INSERT INTO launcher_notes(path,vault,title,body,tags) VALUES(?1,?2,?3,?4,?5)",
            params![path, vault, title, body, tags],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT OR REPLACE INTO launcher_stamps VALUES(?1,?2,?3,?4)",
            params![path, vault, fingerprint, id],
        )?;
        for heading in read
            .prepare("SELECT title,offset FROM headings WHERE path=?1")?
            .query_map([&path], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
        {
            let (title, offset) = heading?;
            tx.execute(
                "INSERT INTO launcher_headings VALUES(?1,?2,?3)",
                params![path, title, offset],
            )?;
        }
        count += 1;
    }
    let previous = tx
        .prepare("SELECT path FROM launcher_stamps WHERE vault=?1")?
        .query_map([vault], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for path in previous {
        if !seen.contains(&path) {
            tx.execute("DELETE FROM launcher_notes WHERE rowid=(SELECT fts_id FROM launcher_stamps WHERE path=?1)",[&path])?;
            tx.execute("DELETE FROM launcher_stamps WHERE path=?1", [&path])?;
            tx.execute("DELETE FROM launcher_headings WHERE path=?1", [&path])?;
        }
    }
    read.commit()?;
    tx.commit()?;
    Ok(count)
}
pub fn launcher_search(db: &Path, query: &str, headings: bool) -> Result<Vec<LauncherHit>> {
    if !db.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if conn
        .query_row(
            "SELECT value FROM launcher_meta WHERE key='enabled'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .as_deref()
        != Some("true")
    {
        return Ok(Vec::new());
    }
    if headings {
        return Ok(conn.prepare("SELECT s.vault,h.path,h.title,h.offset FROM launcher_headings h JOIN launcher_stamps s USING(path) WHERE instr(lower(h.title),lower(?1))>0 ORDER BY h.title LIMIT 30")?.query_map([query],|r|Ok(LauncherHit{vault:PathBuf::from(r.get::<_,String>(0)?),path:PathBuf::from(r.get::<_,String>(1)?),title:r.get(2)?,snippet:"Heading".into(),offset:Some(r.get::<_,i64>(3)?as usize)}))?.collect::<Result<Vec<_>,_>>()?);
    }
    let phrase = query
        .split_whitespace()
        .map(|s| format!("\"{}\"*", s.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
    let sql = if phrase.is_empty() {
        "SELECT vault,path,title,snippet(launcher_notes,3,'','',' … ',12) FROM launcher_notes ORDER BY rowid DESC LIMIT 30"
    } else {
        "SELECT vault,path,title,snippet(launcher_notes,3,'','',' … ',12) FROM launcher_notes WHERE launcher_notes MATCH ?1 ORDER BY rank LIMIT 30"
    };
    let mut stmt = conn.prepare(sql)?;
    let params = if phrase.is_empty() {
        Vec::<String>::new()
    } else {
        vec![phrase]
    };
    Ok(stmt
        .query_map(rusqlite::params_from_iter(params), |r| {
            Ok(LauncherHit {
                vault: PathBuf::from(r.get::<_, String>(0)?),
                path: PathBuf::from(r.get::<_, String>(1)?),
                title: r.get(2)?,
                snippet: r.get(3)?,
                offset: None,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}
pub fn launcher_vaults(db: &Path) -> Result<Vec<PathBuf>> {
    if !db.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if conn
        .query_row(
            "SELECT value FROM launcher_meta WHERE key='enabled'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .as_deref()
        != Some("true")
    {
        return Ok(Vec::new());
    }
    Ok(conn
        .prepare("SELECT path FROM launcher_vaults ORDER BY opened DESC,path")?
        .query_map([], |r| Ok(PathBuf::from(r.get::<_, String>(0)?)))?
        .collect::<Result<Vec<_>, _>>()?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launcher_export_is_opt_in_and_actions_stay_in_vault() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("vault");
        std::fs::create_dir(&root).unwrap();
        let note = root.join("Rust.md");
        std::fs::write(&note, "# Research\nRust memory").unwrap();
        let db = t.path().join("index.sqlite");
        crate::rebuild_index(&db, &root).unwrap();
        let out = t.path().join("launcher.sqlite");
        assert_eq!(export(&db, &root, &out, true).unwrap(), 1);
        assert_eq!(
            launcher_search(&out, "memory", false).unwrap()[0].path,
            note
        );
        assert_eq!(
            launcher_search(&out, "Research", true).unwrap()[0].offset,
            Some(0)
        );
        let action = LauncherAction {
            api_version: 1,
            op: "open".into(),
            vault: root.clone(),
            path: Some(root.join("../escape.md")),
            title: None,
            offset: None,
            append: false,
        };
        assert!(action.validate().is_err());
        export(&db, &root, &out, false).unwrap();
        assert!(launcher_search(&out, "memory", false).unwrap().is_empty());
        assert!(launcher_vaults(&out).unwrap().is_empty());
    }
}
