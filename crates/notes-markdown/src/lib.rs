//! Source ranges are derived metadata; Markdown is never serialized from a syntax tree.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::{collections::BTreeSet, ops::Range};
pub mod editing;
mod metadata;
pub mod render;
pub use metadata::{Metadata, frontmatter};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Heading,
    Strong,
    Emphasis,
    Link,
    Code,
    CodeBlock,
    Strike,
}
#[derive(Clone, Debug)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: Kind,
    pub characters: Range<i32>,
}
#[derive(Clone, Debug)]
pub struct Heading {
    pub text: String,
    pub offset: usize,
    pub level: u8,
}
#[derive(Clone, Debug)]
pub struct Link {
    pub target: String,
    pub range: Range<usize>,
    pub characters: Range<i32>,
    pub wiki: bool,
    pub embed: bool,
}
#[derive(Clone, Debug)]
pub struct Block {
    pub id: String,
    pub offset: usize,
}
#[derive(Clone, Debug)]
pub struct Task {
    pub marker: Range<usize>,
    pub line: Range<usize>,
    pub checked: bool,
}
#[derive(Default, Clone, Debug)]
pub struct Parsed {
    pub spans: Vec<Span>,
    pub headings: Vec<Heading>,
    pub words: usize,
    pub characters: usize,
    pub metadata: Metadata,
    pub links: Vec<Link>,
    pub tags: Vec<String>,
    pub blocks: Vec<Block>,
    pub tasks: Vec<Task>,
    pub hidden: Vec<Range<i32>>,
    pub reading: Vec<render::ReadBlock>,
    /// The front matter's characters (not bytes), for editors that style it.
    pub frontmatter: Option<Range<i32>>,
}

pub fn parse(source: &str) -> Parsed {
    let metadata = metadata::frontmatter(source);
    let body_start = metadata.range.as_ref().map_or(0, |r| r.end);
    let mut tags = metadata.tags.iter().cloned().collect::<BTreeSet<_>>();
    let frontmatter = metadata
        .range
        .as_ref()
        .map(|r| 0..source[..r.end].chars().count() as i32);
    let mut result = Parsed {
        metadata,
        frontmatter,
        words: source.split_whitespace().count(),
        characters: source.chars().count(),
        ..Default::default()
    };
    let mut heading: Option<Heading> = None;
    let mut code_depth = 0usize;
    let mut link_depth = 0usize;
    for (event, range) in Parser::new_ext(
        &source[body_start..],
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_WIKILINKS,
    )
    .into_offset_iter()
    {
        let range = range.start + body_start..range.end + body_start;
        let kind = match &event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some(Heading {
                    text: String::new(),
                    offset: range.start,
                    level: *level as u8,
                });
                Some(Kind::Heading)
            }
            Event::Start(Tag::Strong) => Some(Kind::Strong),
            Event::Start(Tag::Strikethrough) => Some(Kind::Strike),
            Event::Start(Tag::Emphasis) => Some(Kind::Emphasis),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            })
            | Event::Start(Tag::Image {
                link_type,
                dest_url,
                ..
            }) => {
                link_depth += 1;
                result.links.push(Link {
                    target: dest_url.to_string(),
                    range: range.clone(),
                    characters: 0..0,
                    wiki: matches!(link_type, pulldown_cmark::LinkType::WikiLink { .. }),
                    embed: matches!(&event, Event::Start(Tag::Image { .. })),
                });
                Some(Kind::Link)
            }
            Event::Start(Tag::CodeBlock(_)) => {
                code_depth += 1;
                Some(Kind::CodeBlock)
            }
            Event::Code(_) => Some(Kind::Code),
            _ => None,
        };
        if let Some(kind) = kind {
            result.spans.push(Span {
                range: range.clone(),
                kind,
                characters: 0..0,
            });
        }
        if matches!(event, Event::Text(_)) && code_depth == 0 && link_depth == 0 {
            scan_text(source, range.clone(), &mut tags, &mut result.blocks);
        }
        match event {
            Event::Text(text) | Event::Code(text) => {
                if let Some(h) = &mut heading {
                    h.text.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => code_depth = code_depth.saturating_sub(1),
            Event::End(TagEnd::Link | TagEnd::Image) => link_depth = link_depth.saturating_sub(1),
            Event::End(TagEnd::Heading(_)) => {
                if let Some(h) = heading.take() {
                    result.headings.push(h);
                }
            }
            _ => {}
        }
    }
    for (event, range) in
        Parser::new_ext(&source[body_start..], Options::ENABLE_TASKLISTS).into_offset_iter()
    {
        if let Event::TaskListMarker(checked) = event {
            let at = range.start + body_start;
            let start = source[..at].rfind('\n').map_or(0, |n| n + 1);
            let end = source[at..].find('\n').map_or(source.len(), |n| at + n);
            result.tasks.push(Task {
                marker: at + 1..at + 2,
                line: start..end,
                checked,
            });
        }
    }
    result.reading = render::blocks(source);
    result.tags = tags.into_iter().collect();
    map_character_ranges(source, &mut result);
    for span in &result.spans {
        let raw = &source[span.range.clone()];
        let base = span.characters.start;
        if span.kind == Kind::CodeBlock
            && (raw.trim_start().starts_with("```") || raw.trim_start().starts_with("~~~"))
        {
            if let Some(end) = raw.find('\n') {
                result
                    .hidden
                    .push(base..base + character_offset(raw, end + 1));
            }
            let trimmed = raw.trim_end_matches(['\r', '\n']);
            if let Some(start) = trimmed.rfind('\n')
                && (trimmed[start + 1..].trim_start().starts_with("```")
                    || trimmed[start + 1..].trim_start().starts_with("~~~"))
            {
                result
                    .hidden
                    .push(base + character_offset(raw, start + 1)..span.characters.end);
            }
        }
        let (left, right) = match span.kind {
            Kind::Heading => {
                let n = raw.bytes().take_while(|b| *b == b'#').count();
                (
                    if n > 0 {
                        n + usize::from(raw.as_bytes().get(n) == Some(&b' '))
                    } else {
                        0
                    },
                    0,
                )
            }
            Kind::Strong | Kind::Strike => (2, 2),
            Kind::Emphasis => (1, 1),
            Kind::Code => {
                let n = raw.bytes().take_while(|b| *b == b'`').count();
                (n, n)
            }
            _ => (0, 0),
        };
        if left > 0 && raw.len() >= left + right {
            result.hidden.push(base..base + left as i32);
            if right > 0 {
                result
                    .hidden
                    .push(span.characters.end - right as i32..span.characters.end);
            }
        }
    }
    for link in &result.links {
        let raw = &source[link.range.clone()];
        if link.wiki {
            let start = if link.embed { 3 } else { 2 };
            if let Some(pipe) = raw.find('|') {
                result.hidden.push(
                    link.characters.start..link.characters.start + character_offset(raw, pipe + 1),
                );
            } else {
                result
                    .hidden
                    .push(link.characters.start..link.characters.start + start);
            }
            result
                .hidden
                .push(link.characters.end - 2..link.characters.end);
        } else if let Some(close) = raw.find("](") {
            result.hidden.push(
                link.characters.start..link.characters.start + if link.embed { 2 } else { 1 },
            );
            result
                .hidden
                .push(link.characters.start + character_offset(raw, close)..link.characters.end);
        }
    }

    result
}

fn scan_text(
    source: &str,
    range: Range<usize>,
    tags: &mut BTreeSet<String>,
    blocks: &mut Vec<Block>,
) {
    let text = &source[range.clone()];
    for (offset, _) in text.match_indices('#') {
        let absolute = range.start + offset;
        let prefix = &source[..absolute];
        let previous = prefix.chars().next_back();
        if previous
            .is_some_and(|c| !c.is_whitespace() && !matches!(c, '(' | '[' | '{' | '>' | '"' | '\''))
        {
            continue;
        }
        let tag = text[offset + 1..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
            .collect::<String>();
        if !tag.is_empty()
            && !tag.starts_with('/')
            && !tag.ends_with('/')
            && !tag.contains("//")
            && tag.chars().any(|c| !c.is_ascii_digit())
        {
            tags.insert(tag);
        }
    }
    for (offset, _) in text.match_indices('^') {
        let absolute = range.start + offset;
        if absolute > 0
            && !source[..absolute]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            continue;
        }
        let rest = &text[offset + 1..];
        let id = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>();
        if !id.is_empty()
            && source[absolute + 1 + id.len()..]
                .split('\n')
                .next()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            blocks.push(Block {
                id,
                offset: absolute,
            });
        }
    }
}
/// Compute only annotation boundaries on the parser worker, without a per-byte table on GTK.
fn map_character_ranges(source: &str, parsed: &mut Parsed) {
    let mut boundaries = Vec::with_capacity((parsed.spans.len() + parsed.links.len()) * 2);
    for (i, span) in parsed.spans.iter().enumerate() {
        boundaries.push((span.range.start, i, false, false));
        boundaries.push((span.range.end, i, true, false));
    }
    for (i, link) in parsed.links.iter().enumerate() {
        boundaries.push((link.range.start, i, false, true));
        boundaries.push((link.range.end, i, true, true));
    }
    boundaries.sort_unstable_by_key(|b| b.0);
    let mut characters = source.char_indices().peekable();
    let mut count = 0i32;
    for (byte, index, end, link) in boundaries {
        while characters.peek().is_some_and(|(offset, _)| *offset < byte) {
            characters.next();
            count += 1;
        }
        let range = if link {
            &mut parsed.links[index].characters
        } else {
            &mut parsed.spans[index].characters
        };
        if end {
            range.end = count;
        } else {
            range.start = count;
        }
    }
}

/// GTK uses Unicode scalar offsets, while the parser uses UTF-8 byte offsets.
pub fn character_offset(source: &str, byte: usize) -> i32 {
    let mut byte = byte.min(source.len());
    while !source.is_char_boundary(byte) {
        byte -= 1;
    }
    source[..byte]
        .chars()
        .count()
        .try_into()
        .unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn obsidian_metadata_links_and_exclusions() {
        let source = "---\r\naliases: [Research, 'A title']\r\ntags: [project/rust, notes]\r\nstatus: draft\r\n---\r\n# Café 🦀\r\n[[Other#Heading|label]] ![[Chapter#^block]] [md](../Other.md#Heading)\nTag #nested/tag and escaped \\#not-tag.\nText ^block-id\n`[[inline]] #inline`\n```md\n[[fenced]] #fenced\n```\n";
        let parsed = parse(source);
        assert_eq!(parsed.headings.len(), 1);
        assert_eq!(parsed.headings[0].text, "Café 🦀");
        assert_eq!(parsed.metadata.aliases, ["Research", "A title"]);
        assert_eq!(parsed.tags, ["nested/tag", "notes", "project/rust"]);
        assert_eq!(parsed.links.len(), 3, "{:?}", parsed.links);
        assert_eq!(parsed.links[0].target, "Other#Heading");
        assert!(parsed.links[1].embed);
        assert_eq!(parsed.links[1].target, "Chapter#^block");
        assert!(!parsed.links[2].wiki);
        assert_eq!(parsed.blocks[0].id, "block-id");
        for span in &parsed.spans {
            assert_eq!(
                span.characters.start,
                character_offset(source, span.range.start)
            );
            assert_eq!(
                span.characters.end,
                character_offset(source, span.range.end)
            );
        }
        let malformed = parse("---\nkey: [broken\n---\n# Still a heading\n[[Note]]");
        assert!(malformed.metadata.error.is_some());
        assert_eq!(malformed.headings[0].text, "Still a heading");
        assert_eq!(malformed.links[0].target, "Note");
        let unclosed = parse("---\n# Ordinary Markdown\n[[Note]]");
        assert_eq!(unclosed.headings[0].text, "Ordinary Markdown");
        assert_eq!(unclosed.links[0].target, "Note");
    }
    #[test]
    fn ranges_and_unicode_headings() {
        let text = "# Café 🦀\n\n**bold** and *italic* [link](note.md) `code`\n\n```rust\nfn main() {}\n```\n";
        let parsed = parse(text);
        assert_eq!(parsed.headings[0].text, "Café 🦀");
        for kind in [
            Kind::Heading,
            Kind::Strong,
            Kind::Emphasis,
            Kind::Link,
            Kind::Code,
            Kind::CodeBlock,
        ] {
            assert!(parsed.spans.iter().any(|s| s.kind == kind));
        }
        assert_eq!(character_offset("é🦀x", 6), 2);
        assert!(
            parsed
                .spans
                .iter()
                .all(|s| text.get(s.range.clone()).is_some())
        );
    }
}
