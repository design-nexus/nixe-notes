//! JSON Canvas 1.0; preserve extension attributes and use conditional vault saves.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Canvas {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    #[serde(rename = "fromNode")]
    pub from: String,
    #[serde(rename = "toNode")]
    pub to: String,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}
impl Node {
    pub fn field(&self, key: &str) -> &str {
        self.fields.get(key).and_then(Value::as_str).unwrap_or("")
    }
    pub fn set(&mut self, key: &str, value: &str) {
        self.fields.insert(key.into(), Value::String(value.into()));
    }
}
pub fn id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "{:x}{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
impl Canvas {
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > 32 * 1024 * 1024 {
            bail!("Canvas exceeds 32 MiB");
        }
        let canvas: Self = serde_json::from_str(text)?;
        canvas.validate()?;
        Ok(canvas)
    }
    pub fn validate(&self) -> Result<()> {
        if self.nodes.len() > 100000 || self.edges.len() > 500000 {
            bail!("Canvas exceeds supported card/edge limits");
        }
        let mut ids = HashSet::new();
        for node in &self.nodes {
            if node.id.is_empty() || !ids.insert(&node.id) {
                bail!("Duplicate or empty canvas node ID");
            }
            if node.width <= 0
                || node.height <= 0
                || node.width > 1000000
                || node.height > 1000000
                || node.x.unsigned_abs() > 1000000000
                || node.y.unsigned_abs() > 1000000000
            {
                bail!("Invalid canvas geometry");
            }
        }
        let mut edges = HashSet::new();
        for edge in &self.edges {
            if edge.id.is_empty()
                || !edges.insert(&edge.id)
                || !ids.contains(&edge.from)
                || !ids.contains(&edge.to)
            {
                bail!("Invalid canvas connection");
            }
        }
        Ok(())
    }
    pub fn add(&mut self, kind: &str, value: &str, x: i64, y: i64) -> String {
        let mut node = Node {
            id: id(),
            kind: kind.into(),
            x,
            y,
            width: 320,
            height: 220,
            fields: BTreeMap::new(),
        };
        node.set(
            match kind {
                "text" => "text",
                "file" => "file",
                "link" => "url",
                _ => "label",
            },
            value,
        );
        let id = node.id.clone();
        if kind == "group" {
            node.width = 700;
            node.height = 500;
            self.nodes.insert(0, node);
        } else {
            self.nodes.push(node);
        }
        id
    }
    pub fn remove(&mut self, id: &str) {
        self.nodes.retain(|n| n.id != id);
        self.edges
            .retain(|e| e.from != id && e.to != id && e.id != id);
    }
    pub fn connect(&mut self, from: &str, to: &str) -> Result<()> {
        if from == to
            || !self.nodes.iter().any(|n| n.id == from)
            || !self.nodes.iter().any(|n| n.id == to)
        {
            bail!("Select two different cards");
        }
        self.edges.push(Edge {
            id: id(),
            from: from.into(),
            to: to.into(),
            fields: BTreeMap::new(),
        });
        Ok(())
    }
}
pub fn load(root: &Path, path: &Path) -> Result<(Canvas, crate::Loaded)> {
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root) || path.components().any(|c| c.as_os_str() == ".obsidian") {
        bail!("Canvas must be inside the vault");
    }
    let loaded = crate::load(path)?;
    let canvas = Canvas::parse(&loaded.text)?;
    Ok((canvas, loaded))
}
pub fn save(
    root: &Path,
    path: &Path,
    canvas: &Canvas,
    bom: bool,
    expected: &crate::Version,
) -> Result<crate::Version> {
    canvas.validate()?;
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root)
        || path.components().any(|c| c.as_os_str() == ".obsidian")
        || std::fs::symlink_metadata(path)?.file_type().is_symlink()
    {
        bail!("Canvas is read-only");
    }
    let text = serde_json::to_string_pretty(canvas).context("Encode JSON Canvas")?;
    crate::save(path, &text, bom, expected).map_err(|e| anyhow::anyhow!("{e}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn open_format_extensions_and_conflicts() {
        let mut canvas=Canvas::parse(r##"{"nodes":[{"id":"a","type":"text","x":-20,"y":30,"width":300,"height":200,"text":"**hello**","plugin":{"keep":true}}],"edges":[],"custom":"keep"}"##).unwrap();
        let b = canvas.add("file", "Note.md", 400, 0);
        canvas.connect("a", &b).unwrap();
        let json = serde_json::to_string(&canvas).unwrap();
        assert!(json.contains("plugin"));
        assert!(json.contains("custom"));
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("Map.canvas");
        std::fs::write(&path, &json).unwrap();
        let (_, loaded) = load(t.path(), &path).unwrap();
        std::fs::write(&path, "external").unwrap();
        assert!(save(t.path(), &path, &canvas, false, &loaded.version).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
        canvas.remove("a");
        assert!(canvas.edges.is_empty());
    }
}
