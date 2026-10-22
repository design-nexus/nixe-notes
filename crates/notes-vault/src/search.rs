use crate::{children, is_markdown, load};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub path: PathBuf,
    pub snippet: String,
    pub offset: Option<usize>,
}
pub(crate) fn connect(db: &Path) -> Result<Connection> {
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != 5 {
        // The index is derived data; invalidate older schemas rather than retaining stale row mappings.
        conn.execute_batch("BEGIN IMMEDIATE;
            DROP TABLE IF EXISTS notes; DROP TABLE IF EXISTS stamps;
            DROP TABLE IF EXISTS links; DROP TABLE IF EXISTS identities; DROP TABLE IF EXISTS note_tags; DROP TABLE IF EXISTS properties; DROP TABLE IF EXISTS headings;
            CREATE VIRTUAL TABLE notes USING fts5(path UNINDEXED, title, headings, body, tags, properties, tokenize='unicode61');
            CREATE TABLE stamps(path TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, fts_rowid INTEGER NOT NULL,modified INTEGER NOT NULL); CREATE INDEX stamps_modified ON stamps(modified);
            CREATE TABLE identities(name TEXT NOT NULL,path TEXT NOT NULL,PRIMARY KEY(name,path)); CREATE INDEX identities_path ON identities(path);
            CREATE TABLE links(source TEXT NOT NULL,target TEXT NOT NULL,lookup TEXT NOT NULL,wiki INTEGER NOT NULL,embed INTEGER NOT NULL,offset INTEGER NOT NULL,PRIMARY KEY(source,offset)); CREATE INDEX links_lookup ON links(lookup);
            CREATE TABLE note_tags(path TEXT NOT NULL,tag TEXT NOT NULL,PRIMARY KEY(path,tag)); CREATE INDEX tags_lookup ON note_tags(tag,path);
            CREATE TABLE properties(path TEXT NOT NULL,name TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(path,name)); CREATE INDEX properties_lookup ON properties(name,value);
            CREATE TABLE headings(path TEXT NOT NULL,title TEXT NOT NULL,offset INTEGER NOT NULL);CREATE INDEX headings_lookup ON headings(title); CREATE INDEX headings_path ON headings(path,offset);
            PRAGMA user_version=5; COMMIT;")?;
    }
    conn.create_scalar_function(
        "note_number",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let value: String = ctx.get(0)?;
            Ok(value.parse::<f64>().ok().filter(|v| v.is_finite()))
        },
    )?;
    Ok(conn)
}
fn index(conn: &Connection, path: &Path) -> Result<()> {
    let key = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Search requires a UTF-8 filename"))?;
    remove(conn, key)?;
    if !path.exists() {
        return Ok(());
    }
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Ok(());
    }
    let fingerprint = fingerprint(path)?;
    let doc = load(path)?;
    let parsed = notes_markdown::parse(&doc.text);
    let title = path.file_stem().unwrap_or_default().to_string_lossy();
    let headings = parsed
        .headings
        .iter()
        .map(|h| h.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let properties = parsed
        .metadata
        .properties
        .iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let tags = parsed.tags.join(" ");
    let aliases = parsed.metadata.aliases.join(" ");
    conn.execute(
        "INSERT INTO notes(path,title,headings,body,tags,properties) VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            key,
            format!("{title} {aliases}"),
            headings,
            doc.text,
            tags,
            properties
        ],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO stamps(path,fingerprint,fts_rowid,modified) VALUES(?1,?2,?3,?4)",
        params![
            key,
            fingerprint,
            conn.last_insert_rowid(),
            fingerprint
                .split(':')
                .nth(2)
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0)
        ],
    )?;
    for name in
        std::iter::once(title.as_ref()).chain(parsed.metadata.aliases.iter().map(String::as_str))
    {
        conn.execute(
            "INSERT OR IGNORE INTO identities(name,path) VALUES(?1,?2)",
            params![name, key],
        )?;
    }
    for link in &parsed.links {
        if let Ok((target, _)) = crate::links::local_target(&link.target, link.wiki) {
            let lookup = crate::links::note_key(&target);
            conn.execute("INSERT INTO links(source,target,lookup,wiki,embed,offset) VALUES(?1,?2,?3,?4,?5,?6)",params![key,link.target,lookup,link.wiki,link.embed,link.range.start as i64])?;
        }
    }
    for (name, value) in &parsed.metadata.properties {
        conn.execute(
            "INSERT OR REPLACE INTO properties(path,name,value) VALUES(?1,?2,?3)",
            params![key, name, value],
        )?;
    }
    for heading in &parsed.headings {
        conn.execute(
            "INSERT INTO headings(path,title,offset) VALUES(?1,?2,?3)",
            params![key, heading.text, heading.offset as i64],
        )?;
    }
    for tag in &parsed.tags {
        conn.execute(
            "INSERT OR IGNORE INTO note_tags(path,tag) VALUES(?1,?2)",
            params![key, tag.to_lowercase()],
        )?;
    }
    Ok(())
}
fn remove(conn: &Connection, key: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM notes WHERE rowid=(SELECT fts_rowid FROM stamps WHERE path=?1)",
        [key],
    )?;
    conn.execute("DELETE FROM stamps WHERE path=?1", [key])?;
    conn.execute("DELETE FROM links WHERE source=?1", [key])?;
    conn.execute("DELETE FROM identities WHERE path=?1", [key])?;
    conn.execute("DELETE FROM note_tags WHERE path=?1", [key])?;
    conn.execute("DELETE FROM properties WHERE path=?1", [key])?;
    conn.execute("DELETE FROM headings WHERE path=?1", [key])?;
    Ok(())
}
fn fingerprint(path: &Path) -> Result<String> {
    let m = std::fs::symlink_metadata(path)?;
    Ok(format!(
        "{}:{}:{}:{}:{}:{}",
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec()
    ))
}
pub fn rebuild_index(db: &Path, root: &Path) -> Result<(usize, usize)> {
    scan_index(db, root, true)
}
pub fn reconcile_index(db: &Path, root: &Path) -> Result<(usize, usize)> {
    scan_index(db, root, false)
}
fn scan_index(db: &Path, root: &Path, rebuild: bool) -> Result<(usize, usize)> {
    let mut conn = connect(db)?;
    let tx = conn.transaction()?;
    if rebuild {
        tx.execute("DELETE FROM notes", [])?;
        tx.execute("DELETE FROM stamps", [])?;
        tx.execute("DELETE FROM links", [])?;
        tx.execute("DELETE FROM identities", [])?;
        tx.execute("DELETE FROM note_tags", [])?;
        tx.execute("DELETE FROM properties", [])?;
        tx.execute("DELETE FROM headings", [])?;
    }
    let mut seen = HashSet::new();
    let mut pending = vec![root.to_owned()];
    let (mut count, mut skipped) = (0, 0);
    while let Some(dir) = pending.pop() {
        for entry in children(root, &dir)? {
            if entry.symlink {
                continue;
            }
            if entry.directory {
                pending.push(entry.path);
            } else if is_markdown(&entry.path) {
                let Some(key) = entry.path.to_str() else {
                    skipped += 1;
                    continue;
                };
                seen.insert(key.to_owned());
                let stamp = tx
                    .query_row(
                        "SELECT fingerprint FROM stamps WHERE path=?1",
                        [key],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                if stamp.is_some() && fingerprint(&entry.path).ok() == stamp {
                    count += 1;
                    continue;
                }
                match index(&tx, &entry.path) {
                    Ok(()) => count += 1,
                    Err(_) => skipped += 1,
                }
            }
        }
    }
    let previous = tx
        .prepare("SELECT path FROM stamps")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for path in previous {
        if !seen.contains(&path) {
            remove(&tx, &path)?;
        }
    }
    tx.commit()?;
    Ok((count, skipped))
}
pub fn update_index(db: &Path, path: &Path) -> Result<()> {
    let mut conn = connect(db)?;
    let tx = conn.transaction()?;
    index(&tx, path)?;
    tx.commit()?;
    Ok(())
}
fn query_tokens(query: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for ch in query.chars() {
        if ch == '"' {
            quoted = !quoted;
        } else if ch.is_whitespace() && !quoted {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if quoted {
        anyhow::bail!("Close the quoted search phrase");
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    Ok(tokens)
}
// Disjunctions are split only outside phrases; each branch retains the indexed query path.
fn disjunctions(query: &str) -> Result<Vec<&str>> {
    let mut quoted = false;
    let mut start = 0;
    let mut output = Vec::new();
    let chars = query.char_indices().collect::<Vec<_>>();
    for (i, (byte, ch)) in chars.iter().copied().enumerate() {
        if ch == '"' {
            quoted = !quoted;
        }
        if !quoted
            && query[byte..].starts_with("OR")
            && (i == 0 || chars[i - 1].1.is_whitespace())
            && query[byte + 2..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
        {
            let part = query[start..byte].trim();
            if part.is_empty() {
                anyhow::bail!("OR needs a search on each side");
            }
            output.push(part);
            start = byte + 2;
        }
    }
    if quoted {
        anyhow::bail!("Close the quoted search phrase");
    }
    let last = query[start..].trim();
    if last.is_empty() && !output.is_empty() {
        anyhow::bail!("OR needs a search on each side");
    }
    output.push(last);
    Ok(output)
}
pub fn search(db: &Path, query: &str) -> Result<Vec<SearchHit>> {
    query_index(db, query, 100)
}
pub(crate) fn search_all(db: &Path, query: &str) -> Result<Vec<SearchHit>> {
    query_index(db, query, -1)
}
fn query_index(db: &Path, query: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let groups = disjunctions(query)?;
    if groups.len() > 1 {
        let mut seen = std::collections::HashSet::new();
        let mut hits = Vec::new();
        for group in groups {
            for hit in query_index(db, group, limit)? {
                if seen.insert(hit.path.clone()) {
                    hits.push(hit);
                    if limit >= 0 && hits.len() >= limit as usize {
                        return Ok(hits);
                    }
                }
            }
        }
        return Ok(hits);
    }
    let conn = connect(db)?;
    let mut clauses = Vec::new();
    let mut values = Vec::<String>::new();
    let mut words = Vec::new();
    let mut heading_query = None;
    for token in query_tokens(query)? {
        let (negative, token) = token
            .strip_prefix('-')
            .map_or((false, token.as_str()), |s| (true, s));
        let filter = if let Some(tag) = token.strip_prefix("tag:") {
            values.push(tag.trim_start_matches('#').to_lowercase());
            format!(
                "EXISTS(SELECT 1 FROM note_tags t WHERE t.path=n.path AND (t.tag=?{i} OR substr(t.tag,1,length(?{i})+1)=?{i}||'/'))",
                i = values.len()
            )
        } else if let Some(path) = token.strip_prefix("path:") {
            values.push(path.to_owned());
            format!("instr(n.path,?{})>0", values.len())
        } else if token.starts_with("before:") || token.starts_with("after:") {
            let (op, date) = if let Some(date) = token.strip_prefix("before:") {
                ("<", date)
            } else {
                (">=", token.strip_prefix("after:").unwrap())
            };
            let time = gio::glib::DateTime::from_iso8601(&format!("{date}T00:00:00Z"), None)
                .map_err(|_| anyhow::anyhow!("Use an ISO date, for example after:2026-10-01"))?;
            values.push(time.to_unix().to_string());
            format!(
                "EXISTS(SELECT 1 FROM stamps s WHERE s.path=n.path AND s.modified {op} CAST(?{} AS INTEGER))",
                values.len()
            )
        } else if let Some(heading) = token.strip_prefix("heading:") {
            heading_query = Some(heading.to_owned());
            values.push(heading.to_owned());
            format!(
                "EXISTS(SELECT 1 FROM headings h WHERE h.path=n.path AND instr(lower(h.title),lower(?{}))>0)",
                values.len()
            )
        } else if let Some(property) = token
            .strip_prefix("property:")
            .or_else(|| token.strip_prefix('[').and_then(|s| s.strip_suffix(']')))
        {
            let comparison = [">=", "<=", ">", "<"].iter().find_map(|op| {
                property
                    .split_once(op)
                    .map(|(key, value)| (*op, key, value))
            });
            if let Some((op, name, value)) = comparison {
                let number = value
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Numeric property comparisons need a finite number")
                    })?;
                values.push(name.to_owned());
                let key = values.len();
                values.push(number.to_string());
                format!(
                    "EXISTS(SELECT 1 FROM properties p WHERE p.path=n.path AND p.name=?{key} AND note_number(p.value) {op} CAST(?{} AS REAL))",
                    values.len()
                )
            } else {
                let exact = property.contains('=');
                let (name, value) = property
                    .split_once('=')
                    .or_else(|| property.split_once(':'))
                    .unwrap_or((property, ""));
                values.push(name.to_owned());
                let key = values.len();
                values.push(value.to_owned());
                let test = if exact {
                    format!("lower(p.value)=lower(?{})", values.len())
                } else {
                    format!("instr(lower(p.value),lower(?{}))>0", values.len())
                };
                format!(
                    "EXISTS(SELECT 1 FROM properties p WHERE p.path=n.path AND p.name=?{key} AND {test})"
                )
            }
        } else {
            let escaped = format!("\"{}\"*", token.replace('"', "\"\""));
            words.push(if negative {
                format!("NOT {escaped}")
            } else {
                escaped
            });
            continue;
        };
        clauses.push(if negative {
            format!("NOT ({filter})")
        } else {
            filter
        });
    }
    if !words.is_empty() {
        let positives = words
            .iter()
            .filter(|w| !w.starts_with("NOT "))
            .cloned()
            .collect::<Vec<_>>();
        if positives.is_empty() {
            anyhow::bail!("Start a text search with a positive term");
        }
        let negatives = words
            .iter()
            .filter_map(|w| w.strip_prefix("NOT "))
            .collect::<Vec<_>>();
        let mut expression = positives.join(" AND ");
        if !negatives.is_empty() {
            expression = format!("({expression}) NOT ({})", negatives.join(" OR "));
        }
        values.push(expression);
        clauses.push(format!("notes MATCH ?{}", values.len()));
    }
    let condition = if clauses.is_empty() {
        "1".into()
    } else {
        clauses.join(" AND ")
    };
    let sql = format!(
        "SELECT n.path,snippet(notes,3,'[',']',' … ',18) FROM notes n WHERE {condition} ORDER BY {} LIMIT {limit}",
        if words.is_empty() {
            "rowid DESC"
        } else {
            "rank"
        }
    );
    let mut statement = conn.prepare(&sql)?;
    let mut hits = statement
        .query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok(SearchHit {
                path: PathBuf::from(row.get::<_, String>(0)?),
                snippet: row.get(1)?,
                offset: None,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(query) = heading_query {
        for hit in &mut hits {
            let row=conn.query_row("SELECT title,offset FROM headings WHERE path=?1 AND instr(lower(title),lower(?2))>0 ORDER BY offset LIMIT 1",params![hit.path.to_string_lossy(),query],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?))).optional()?;
            if let Some((title, offset)) = row {
                hit.snippet = title;
                hit.offset = Some(offset as usize);
            }
        }
    }
    Ok(hits)
}
pub fn mentions(db: &Path, root: &Path, target: &Path) -> Result<Vec<SearchHit>> {
    let conn = connect(db)?;
    let name = target.file_stem().unwrap_or_default().to_string_lossy();
    let terms = std::iter::once(name.to_string())
        .chain(
            conn.prepare("SELECT name FROM identities WHERE path=?1")?
                .query_map([target.to_string_lossy().as_ref()], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?,
        )
        .collect::<std::collections::BTreeSet<_>>();
    let mut hits = Vec::new();
    for term in terms {
        if term.is_empty() {
            continue;
        }
        for mut hit in search(db, &format!("\"{}\"", term.replace('"', "")))? {
            if hit.path == target || hits.iter().any(|h: &SearchHit| h.path == hit.path) {
                continue;
            }
            let doc = load(&hit.path)?;
            let parsed = notes_markdown::parse(&doc.text);
            if parsed.links.iter().any(|link| {
                crate::links::resolve(&conn, root, &hit.path, &link.target, link.wiki)
                    .is_ok_and(|r| r.path == target)
            }) {
                continue;
            }
            if let Some(offset) = doc.text.find(&term)
                && !parsed.spans.iter().any(|s| {
                    matches!(
                        s.kind,
                        notes_markdown::Kind::Code | notes_markdown::Kind::CodeBlock
                    ) && s.range.contains(&offset)
                })
            {
                hit.offset = Some(offset);
                hits.push(hit);
            }
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rebuild_update_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        let path = vault.join("Rust.md");
        std::fs::write(&path, "# Rust\nSafe memory and café").unwrap();
        let db = dir.path().join("index.sqlite");
        assert_eq!(rebuild_index(&db, &vault).unwrap(), (1, 0));
        assert_eq!(reconcile_index(&db, &vault).unwrap(), (1, 0));
        assert_eq!(search(&db, "memory").unwrap()[0].path, path);
        std::fs::write(&path, "new words").unwrap();
        reconcile_index(&db, &vault).unwrap();
        assert!(search(&db, "memory").unwrap().is_empty());
        assert_eq!(search(&db, "new words").unwrap().len(), 1);
        std::fs::write(
            &path,
            "---\nstatus: draft\nrating: 4.5\ntags: [Project/Rust]\n---\n# Architecture\nnew words\n",
        )
        .unwrap();
        update_index(&db, &path).unwrap();
        assert_eq!(
            search(
                &db,
                "new words tag:project property:status=draft path:Rust heading:Architecture"
            )
            .unwrap()[0]
                .path,
            path
        );
        assert!(search(&db, "new -tag:project").unwrap().is_empty());
        assert!(
            search(&db, "heading:Architecture").unwrap()[0]
                .offset
                .is_some()
        );

        assert_eq!(
            search(&db, "property:rating>=4 after:2000-01-01 before:2099-01-01")
                .unwrap()
                .len(),
            1
        );
        assert!(search(&db, "property:rating<4").unwrap().is_empty());
        assert_eq!(search(&db, "missing OR tag:project").unwrap().len(), 1);
        assert_eq!(
            search(&db, "new words -missing -excluded").unwrap().len(),
            1
        );
        assert!(search(&db, "new -words -missing").unwrap().is_empty());
        assert!(search(&db, "OR tag:project").is_err());
        assert!(search(&db, "after:not-a-date").is_err());
        assert!(search(&db, "property:rating>=NaN").is_err());
        std::fs::remove_file(&path).unwrap();
        update_index(&db, &path).unwrap();
        assert!(search(&db, "new").unwrap().is_empty());
    }
}
