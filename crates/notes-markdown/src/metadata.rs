//! Metadata is derived for display/indexing. It is never written back to Markdown.
use saphyr::{LoadableYamlNode, Yaml, YamlEmitter};
use std::ops::Range;

#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub range: Option<Range<usize>>,
    pub properties: Vec<(String, String)>,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub error: Option<String>,
}
pub fn frontmatter(source: &str) -> Metadata {
    let first = source.lines().next().unwrap_or("");
    if first != "---" {
        return Metadata::default();
    }
    let mut start = 0;
    let mut content_start = 0;
    let mut end = None;
    for (line_number, line) in source.split_inclusive('\n').enumerate() {
        if line_number == 0 {
            content_start = line.len();
        } else if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
            end = Some((start, start + line.len()));
            break;
        }
        start += line.len();
    }
    let Some((content_end, block_end)) = end else {
        return Metadata {
            range: None,
            error: Some("Unclosed YAML frontmatter".into()),
            ..Default::default()
        };
    };
    let mut metadata = Metadata {
        range: Some(0..block_end),
        ..Default::default()
    };
    match Yaml::load_from_str(&source[content_start..content_end]) {
        Ok(documents) => {
            if documents.is_empty() || documents[0].is_null() {
                return metadata;
            }
            if documents.len() != 1 || !documents[0].is_mapping() {
                metadata.error = Some("Frontmatter must be a YAML mapping".into());
                return metadata;
            }
            let yaml = &documents[0];
            for (key, value) in yaml.as_mapping().unwrap() {
                if let Some(name) = key.as_str() {
                    metadata.properties.push((name.to_owned(), display(value)));
                }
            }
            metadata.aliases = yaml
                .as_mapping_get("aliases")
                .map(strings)
                .unwrap_or_default();
            metadata.aliases.extend(
                yaml.as_mapping_get("alias")
                    .map(strings)
                    .unwrap_or_default(),
            );
            metadata.tags = yaml
                .as_mapping_get("tags")
                .map(strings)
                .unwrap_or_default()
                .into_iter()
                .flat_map(|s| {
                    s.split_whitespace()
                        .map(|s| s.trim_start_matches('#').to_owned())
                        .collect::<Vec<_>>()
                })
                .collect();
        }
        Err(error) => metadata.error = Some(format!("Invalid YAML: {error}")),
    }
    metadata
}
fn strings(yaml: &Yaml<'_>) -> Vec<String> {
    if let Some(text) = yaml.as_str() {
        vec![text.to_owned()]
    } else if let Some(sequence) = yaml.as_sequence() {
        sequence
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    } else {
        Vec::new()
    }
}
fn display(yaml: &Yaml<'_>) -> String {
    if let Some(text) = yaml.as_str() {
        return text.to_owned();
    }
    let mut output = String::new();
    if YamlEmitter::new(&mut output).dump(yaml).is_err() {
        return "Unsupported YAML value".into();
    }
    output
        .trim_start_matches("---\n")
        .trim()
        .chars()
        .take(2048)
        .collect()
}
