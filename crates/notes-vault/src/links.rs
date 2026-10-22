//! Link resolution stays read-only and inside the selected vault.
use super::search::connect;
use anyhow::{Result, bail};
use percent_encoding::percent_decode_str;
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ResolvedLink {
    pub path: PathBuf,
    pub fragment: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Backlink {
    pub path: PathBuf,
    pub offset: usize,
    pub embed: bool,
}

pub fn local_target(target: &str, wiki: bool) -> Result<(String, Option<String>)> {
    let (path, fragment) = target
        .split_once('#')
        .map_or((target, None), |(p, f)| (p, Some(f)));
    let decode =
        |s: &str| -> Result<String> { Ok(percent_decode_str(s).decode_utf8()?.into_owned()) };
    let path = if wiki { path.to_owned() } else { decode(path)? };
    // Schemes and network paths are not vault paths. Nothing is executed by resolution.
    if path.starts_with("//")
        || path
            .split('/')
            .next()
            .is_some_and(|part| part.contains(':'))
    {
        bail!("This action opens local Markdown notes only");
    }
    let fragment = fragment.map(decode).transpose()?;
    Ok((path, fragment))
}
pub(crate) fn note_key(target: &str) -> String {
    let name = Path::new(target)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let lower = name.to_lowercase();
    if lower.ends_with(".markdown") {
        name[..name.len() - 9].to_owned()
    } else if lower.ends_with(".md") {
        name[..name.len() - 3].to_owned()
    } else {
        name.into_owned()
    }
}
fn candidate(root: &Path, path: PathBuf) -> Result<Option<PathBuf>> {
    let Ok(canonical) = path.canonicalize() else {
        return Ok(None);
    };
    if !canonical.starts_with(root) {
        bail!("Link points outside this vault");
    }
    if canonical.is_file() && crate::is_markdown(&canonical) {
        // Collapse relative aliases so ordinary links reuse the same document tab.
        // Preserve a final symlink path so the editor still opens it read-only.
        let symlink = std::fs::symlink_metadata(&path)?.file_type().is_symlink();
        return Ok(Some(if symlink { path } else { canonical }));
    }
    Ok(None)
}
fn with_extensions(path: PathBuf) -> Vec<PathBuf> {
    if crate::is_markdown(&path) {
        vec![path]
    } else {
        let mut md = path.as_os_str().to_os_string();
        md.push(".md");
        let mut markdown = path.into_os_string();
        markdown.push(".markdown");
        vec![PathBuf::from(md), PathBuf::from(markdown)]
    }
}
pub(crate) fn resolve(
    conn: &Connection,
    root: &Path,
    source: &Path,
    target: &str,
    wiki: bool,
) -> Result<ResolvedLink> {
    let (target, fragment) = local_target(target, wiki)?;
    if target.is_empty() {
        return Ok(ResolvedLink {
            path: source.to_owned(),
            fragment,
        });
    }
    let raw = PathBuf::from(&target);
    let bases = if raw.is_absolute() {
        vec![raw]
    } else if target.contains('/') && wiki {
        vec![root.join(&raw), source.parent().unwrap_or(root).join(&raw)]
    } else {
        vec![source.parent().unwrap_or(root).join(&raw), root.join(&raw)]
    };
    for base in bases {
        for path in with_extensions(base) {
            if let Some(path) = candidate(root, path)? {
                return Ok(ResolvedLink { path, fragment });
            }
        }
    }
    if target.contains('/') {
        bail!("Note not found: {target}");
    }
    let key = note_key(&target);
    let mut statement = conn.prepare("SELECT path FROM identities WHERE name=?1 ORDER BY path")?;
    let paths = statement
        .query_map([key], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut candidates = Vec::new();
    for path in paths {
        if let Some(path) = candidate(root, PathBuf::from(path))? {
            candidates.push(path);
        }
    }
    match candidates.len() {
        0 => bail!("Note not found: {target}"),
        1 => Ok(ResolvedLink {
            path: candidates.remove(0),
            fragment,
        }),
        _ => bail!(
            "Ambiguous note '{target}'. Use its folder path: {}",
            candidates
                .iter()
                .map(|p| p.strip_prefix(root).unwrap_or(p).display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
pub fn resolve_link(
    db: &Path,
    root: &Path,
    source: &Path,
    target: &str,
    wiki: bool,
) -> Result<ResolvedLink> {
    let root = root.canonicalize()?;
    if !source.canonicalize()?.starts_with(&root) {
        bail!("The source note is outside this vault");
    }
    resolve(&connect(db)?, &root, source, target, wiki)
}
pub fn backlinks(db: &Path, root: &Path, target: &Path) -> Result<Vec<Backlink>> {
    let conn = connect(db)?;
    let root = root.canonicalize()?;
    let target_canonical = target.canonicalize()?;
    let mut statement=conn.prepare("SELECT source,target,wiki,offset,embed FROM links WHERE lookup IN (SELECT name FROM identities WHERE path=?1 UNION SELECT ?2) OR (source=?1 AND lookup='') ORDER BY source,offset")?;
    let rows = statement
        .query_map(
            params![
                target.to_string_lossy(),
                note_key(&target.to_string_lossy())
            ],
            |row| {
                Ok((
                    PathBuf::from(row.get::<_, String>(0)?),
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, i64>(3)? as usize,
                    row.get::<_, bool>(4)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut links = Vec::new();
    for (source, destination, wiki, offset, embed) in rows {
        if let Ok(link) = resolve(&conn, &root, &source, &destination, wiki)
            && link.path.canonicalize().ok().as_ref() == Some(&target_canonical)
        {
            links.push(Backlink {
                path: source,
                offset,
                embed,
            });
        }
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{rebuild_index, search, update_index};
    #[test]
    fn resolves_links_backlinks_aliases_and_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("vault");
        std::fs::create_dir_all(root.join("Research")).unwrap();
        let target = root.join("Research/Café.md");
        std::fs::write(&target,"---\naliases: [Coffee]\ntags: [project/rust]\nstatus: reviewing\n---\n# Intro\nParagraph ^block\n").unwrap();
        let source = root.join("Source.md");
        std::fs::write(&source,"[[Coffee#Intro|alias]] ![[Research/Café#^block]] [link](Research/Caf%C3%A9.md#Intro)\n`[[Coffee]]`\n```\n[[Coffee]]\n```\n").unwrap();
        let db = temp.path().join("index.sqlite");
        rebuild_index(&db, &root).unwrap();
        assert_eq!(
            resolve_link(&db, &root, &source, "Coffee#Intro", true)
                .unwrap()
                .path,
            target
        );
        assert_eq!(
            resolve_link(&db, &root, &source, "Research/Caf%C3%A9.md#Intro", false)
                .unwrap()
                .fragment
                .as_deref(),
            Some("Intro")
        );
        assert_eq!(backlinks(&db, &root, &target).unwrap().len(), 3);
        assert!(
            backlinks(&db, &root, &target)
                .unwrap()
                .iter()
                .any(|l| l.embed)
        );
        assert_eq!(search(&db, "tag:project").unwrap()[0].path, target);
        assert_eq!(search(&db, "reviewing").unwrap()[0].path, target);
        let outside = temp.path().join("outside.md");
        std::fs::write(&outside, "outside").unwrap();
        assert!(resolve_link(&db, &root, &source, "../outside.md", false).is_err());
        assert!(resolve_link(&db, &root, &source, "https://example.com/", false).is_err());
        assert!(resolve_link(&db, &root, &source, "Missing", true).is_err());
        assert_eq!(
            resolve_link(&db, &root, &source, "Research/../Research/Café.md", false)
                .unwrap()
                .path,
            target
        );
        std::fs::write(root.join("Research/New.md"), "# New").unwrap();
        update_index(&db, &root.join("Research/New.md")).unwrap();
        assert!(resolve_link(&db, &root, &source, "New", true).is_ok());
        std::fs::create_dir(root.join("Other")).unwrap();
        std::fs::write(root.join("Other/New.md"), "# Another").unwrap();
        update_index(&db, &root.join("Other/New.md")).unwrap();
        assert!(
            resolve_link(&db, &root, &source, "New", true)
                .unwrap_err()
                .to_string()
                .contains("Ambiguous")
        );
        assert_eq!(
            resolve_link(&db, &root, &source, "Research/New", true)
                .unwrap()
                .path,
            root.join("Research/New.md")
        );
        std::fs::write(&source, "no outgoing links").unwrap();
        update_index(&db, &source).unwrap();
        assert!(backlinks(&db, &root, &target).unwrap().is_empty());
        std::fs::remove_file(&target).unwrap();
        update_index(&db, &target).unwrap();
        assert!(resolve_link(&db, &root, &source, "Coffee", true).is_err());
        assert!(search(&db, "tag:project").unwrap().is_empty());
    }
}
