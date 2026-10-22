//! Derived knowledge models; independent of GTK and user-facing rendering.
use anyhow::Result;
pub use notes_markdown::editing::property_value;
use petgraph::{algo::dijkstra, graphmap::UnGraphMap};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};
#[derive(Clone, Debug)]
pub struct Note {
    pub path: PathBuf,
    pub title: String,
    pub properties: BTreeMap<String, String>,
    pub tags: Vec<String>,
}
pub fn notes(db: &Path, root: &Path) -> Result<Vec<Note>> {
    let conn = crate::search::connect(db)?;
    let paths = conn
        .prepare("SELECT path FROM stamps ORDER BY path")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut notes = Vec::new();
    let mut lookup = HashMap::new();
    for path in paths {
        let path = PathBuf::from(path);
        if !path.starts_with(root) {
            continue;
        }
        lookup.insert(path.to_string_lossy().to_string(), notes.len());
        notes.push(Note {
            title: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path,
            properties: BTreeMap::new(),
            tags: Vec::new(),
        });
    }
    for row in conn
        .prepare("SELECT path,name,value FROM properties")?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
    {
        let (path, key, value) = row?;
        if let Some(&i) = lookup.get(&path) {
            notes[i].properties.insert(key, value);
        }
    }
    for row in conn
        .prepare("SELECT path,tag FROM note_tags")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
    {
        let (path, tag) = row?;
        if let Some(&i) = lookup.get(&path) {
            notes[i].tags.push(tag);
        }
    }
    Ok(notes)
}
#[derive(Clone, Debug)]
pub struct GraphNode {
    pub note: Note,
    pub x: f64,
    pub y: f64,
    pub degree: usize,
    pub group: String,
}
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<(usize, usize)>,
}
impl Graph {
    pub fn load(db: &Path, root: &Path) -> Result<Self> {
        let notes = notes(db, root)?;
        let lookup = notes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.path.clone(), i))
            .collect::<HashMap<_, _>>();
        let conn = crate::search::connect(db)?;
        let mut edges = HashSet::new();
        let rows = conn
            .prepare("SELECT source,target,wiki FROM links")?
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut cache = HashMap::new();
        for (source, target, wiki) in rows {
            if crate::links::local_target(&target, wiki).is_ok_and(|(path, _)| path.is_empty()) {
                continue;
            }
            let source = PathBuf::from(source);
            let Some(&from) = lookup.get(&source) else {
                continue;
            };
            let key = (source.parent().map(PathBuf::from), target.clone(), wiki);
            let to = cache.entry(key).or_insert_with(|| {
                crate::links::resolve(&conn, root, &source, &target, wiki)
                    .ok()
                    .and_then(|r| lookup.get(&r.path).copied())
            });
            if let Some(to) = *to
                && from != to
            {
                edges.insert((from.min(to), from.max(to)));
            }
        }
        let mut groups = BTreeMap::<String, Vec<usize>>::new();
        let mut nodes = Vec::new();
        for (i, note) in notes.into_iter().enumerate() {
            let group = note
                .path
                .strip_prefix(root)?
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Vault".into());
            groups.entry(group.clone()).or_default().push(i);
            nodes.push(GraphNode {
                note,
                x: 0.0,
                y: 0.0,
                degree: 0,
                group,
            });
        }
        let mut edges = edges.into_iter().collect::<Vec<_>>();
        edges.sort_unstable();
        for &(a, b) in &edges {
            nodes[a].degree += 1;
            nodes[b].degree += 1;
        }
        // Stable clustered phyllotaxis avoids O(n²) force layout and needs no recomputation while panning.
        let count = groups.len();
        for (g, (_, members)) in groups.into_iter().enumerate() {
            let angle = g as f64 * std::f64::consts::TAU / count.max(1) as f64;
            let center = if count > 1 {
                (angle.cos() * 650.0, angle.sin() * 650.0)
            } else {
                (0.0, 0.0)
            };
            for (i, index) in members.into_iter().enumerate() {
                let radius = 36.0 * (i as f64).sqrt();
                let theta = i as f64 * 2.399963229728653;
                nodes[index].x = center.0 + radius * theta.cos();
                nodes[index].y = center.1 + radius * theta.sin();
            }
        }
        Ok(Self { nodes, edges })
    }
    pub fn visible(
        &self,
        query: &str,
        tag: &str,
        center: Option<&Path>,
        depth: usize,
        orphans: bool,
    ) -> HashSet<usize> {
        let mut reachable = None;
        if let Some(center) = center {
            if let Some(start) = self.nodes.iter().position(|n| n.note.path == center) {
                let mut graph = UnGraphMap::<usize, ()>::from_edges(self.edges.iter().copied());
                graph.add_node(start);
                let distances = dijkstra(&graph, start, None, |_| 1usize);
                reachable = Some(
                    distances
                        .into_iter()
                        .filter(|(_, d)| *d <= depth)
                        .map(|(i, _)| i)
                        .collect::<HashSet<_>>(),
                );
            } else {
                return HashSet::new();
            }
        }
        let query = query.to_lowercase();
        let tag = tag.trim_start_matches('#').to_lowercase();
        self.nodes
            .iter()
            .enumerate()
            .filter(|(i, n)| {
                reachable.as_ref().is_none_or(|s| s.contains(i))
                    && (orphans || n.degree > 0)
                    && n.note
                        .path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&query)
                    && (tag.is_empty()
                        || n.note
                            .tags
                            .iter()
                            .any(|t| t == &tag || t.starts_with(&format!("{tag}/"))))
            })
            .map(|(i, _)| i)
            .collect()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Collection {
    pub name: String,
    pub query: String,
    pub layout: String,
    pub sort: String,
    pub descending: bool,
    pub group: String,
    pub columns: String,
}
impl Default for Collection {
    fn default() -> Self {
        Self {
            name: "All notes".into(),
            query: String::new(),
            layout: "table".into(),
            sort: "title".into(),
            descending: false,
            group: "status".into(),
            columns: "status,tags".into(),
        }
    }
}
pub fn collection(db: &Path, root: &Path, view: &Collection) -> Result<Vec<Note>> {
    let mut all = notes(db, root)?;
    if !view.query.trim().is_empty() {
        // Metadata predicates retain the same query language as search; collect all pages, not the UI's first 100 hits.
        let hits = crate::search::search_all(db, &view.query)?;
        let paths = hits.into_iter().map(|h| h.path).collect::<HashSet<_>>();
        all.retain(|n| paths.contains(&n.path));
    }
    all.sort_by(|a, b| {
        let value = |n: &Note| {
            if view.sort == "title" {
                n.title.to_lowercase()
            } else if view.sort == "path" {
                n.path.to_string_lossy().to_lowercase()
            } else {
                n.properties.get(&view.sort).cloned().unwrap_or_default()
            }
        };
        let left = value(a);
        let right = value(b);
        let order = match (left.parse::<f64>(), right.parse::<f64>()) {
            (Ok(a), Ok(b)) if a.is_finite() && b.is_finite() => a.total_cmp(&b),
            _ => left.to_lowercase().cmp(&right.to_lowercase()),
        }
        .then(a.path.cmp(&b.path));
        if view.descending {
            order.reverse()
        } else {
            order
        }
    });
    Ok(all)
}
pub fn set_property(
    state: &Path,
    root: &Path,
    path: &Path,
    key: &str,
    value: Option<&str>,
    keep: u32,
    days: u32,
) -> Result<()> {
    edit_property(state, root, path, key, value, (keep, days), None)
}
pub fn edit_property(
    state: &Path,
    root: &Path,
    path: &Path,
    key: &str,
    value: Option<&str>,
    retention: (u32, u32),
    expected: Option<&crate::Version>,
) -> Result<()> {
    let (keep, days) = retention;
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root)
        || path.components().any(|c| c.as_os_str() == ".obsidian")
        || std::fs::symlink_metadata(path)?.file_type().is_symlink()
    {
        anyhow::bail!("Choose a writable note inside the vault");
    }
    let loaded = crate::load(path)?;
    if expected.is_some_and(|v| v.digest != loaded.version.digest || v.etag != loaded.version.etag)
    {
        anyhow::bail!(
            "The note changed while the property editor was open. Reopen it before applying."
        );
    }
    let edit = notes_markdown::editing::set_property(&loaded.text, key, value)
        .map_err(anyhow::Error::msg)?;
    let mut text = loaded.text.clone();
    text.replace_range(edit.range, &edit.replacement);
    crate::history::capture(state, path, &loaded.text, loaded.bom, keep, days)?;
    crate::history::capture(state, path, &text, loaded.bom, keep, days)?;
    crate::save(path, &text, loaded.bom, &loaded.version).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub name: String,
    pub root: PathBuf,
    pub chapters: Vec<PathBuf>,
    pub goal: usize,
}
impl Default for Project {
    fn default() -> Self {
        Self {
            name: "Writing project".into(),
            root: PathBuf::new(),
            chapters: Vec::new(),
            goal: 50000,
        }
    }
}
pub fn manuscript(project: &Project) -> Result<(String, usize)> {
    let mut output = String::new();
    let mut words = 0;
    for path in &project.chapters {
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(&project.root) {
            anyhow::bail!("Chapter outside project vault");
        }
        let text = crate::load(path)?.text;
        let meta = notes_markdown::frontmatter(&text);
        let start = meta.range.map_or(0, |r| r.end);
        let body = &text[start..];
        words += body.split_whitespace().count();
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(body);
    }
    Ok((output, words))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graph_collections_and_manuscripts_are_derived() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("vault");
        std::fs::create_dir(&root).unwrap();
        let a = root.join("A.md");
        let b = root.join("B.md");
        std::fs::write(
            &a,
            "---\nstatus: draft\ntags: [work/research]\n---\n# A\n[[B]]",
        )
        .unwrap();
        std::fs::write(&b, "# B\ntext").unwrap();
        let db = t.path().join("index.sqlite");
        crate::rebuild_index(&db, &root).unwrap();
        let graph = Graph::load(&db, &root).unwrap();
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.visible("", "", Some(&a), 1, true).len(), 2);
        assert_eq!(graph.visible("", "work", None, 1, true).len(), 1);
        assert_eq!(graph.visible("", "", Some(&a), 0, true).len(), 1);
        let view = Collection {
            query: "property:status=draft".into(),
            ..Default::default()
        };
        assert_eq!(collection(&db, &root, &view).unwrap().len(), 1);
        set_property(t.path(), &root, &a, "status", Some("done"), 10, 30).unwrap();
        let stale = crate::load(&a).unwrap().version;
        set_property(t.path(), &root, &a, "status", Some("review"), 10, 30).unwrap();
        assert!(
            edit_property(
                t.path(),
                &root,
                &a,
                "status",
                Some("stale"),
                (10, 30),
                Some(&stale)
            )
            .is_err()
        );
        crate::update_index(&db, &a).unwrap();
        assert!(collection(&db, &root, &view).unwrap().is_empty());
        assert!(std::fs::read_to_string(&a).unwrap().ends_with("# A\n[[B]]"));
        let (text, words) = manuscript(&Project {
            root: root.clone(),
            chapters: vec![a, b],
            ..Default::default()
        })
        .unwrap();
        assert!(!text.contains("status:"));
        assert!(text.contains("# B"));
        assert!(words > 0);
    }
}
