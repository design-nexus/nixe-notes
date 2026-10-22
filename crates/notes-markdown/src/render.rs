//! Native-reader blocks. Markup is escaped and contains only Pango-supported tags.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
#[derive(Clone, Debug, PartialEq)]
pub enum ReadKind {
    Text,
    Heading(u8),
    Code(String),
    Quote,
    Table,
    Rule,
    Embed { target: String, wiki: bool },
    Task { checked: bool, offset: usize },
    Math,
}
#[derive(Clone, Debug)]
pub struct ReadBlock {
    pub offset: usize,
    pub kind: ReadKind,
    pub markup: String,
    pub text: String,
}
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub fn blocks(source: &str) -> Vec<ReadBlock> {
    let meta = crate::metadata::frontmatter(source);
    let base = meta.range.map_or(0, |r| r.end);
    let mut result = Vec::new();
    let mut text = String::new();
    let mut markup = String::new();
    let mut kind = ReadKind::Text;
    let mut offset = base;
    let mut code = false;
    let mut quote = 0usize;
    let mut image = 0usize;
    let mut list = Vec::<Option<u64>>::new();
    fn flush(
        result: &mut Vec<ReadBlock>,
        text: &mut String,
        markup: &mut String,
        kind: &ReadKind,
        offset: usize,
    ) {
        if !text.trim().is_empty() {
            result.push(ReadBlock {
                offset,
                kind: kind.clone(),
                text: std::mem::take(text),
                markup: std::mem::take(markup),
            });
        }
    }
    for (event, range) in Parser::new_ext(
        &source[base..],
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_WIKILINKS
            | Options::ENABLE_MATH,
    )
    .into_offset_iter()
    {
        let at = range.start + base;
        if image > 0 {
            if matches!(event, Event::End(TagEnd::Image)) {
                image -= 1;
            }
            continue;
        }
        match event {
            Event::Start(Tag::Paragraph) => {
                if text != "• " && !text.trim_end().ends_with('.') {
                    flush(&mut result, &mut text, &mut markup, &kind, offset);
                }
                offset = at;
                if !matches!(kind, ReadKind::Task { .. }) {
                    kind = if quote > 0 {
                        ReadKind::Quote
                    } else {
                        ReadKind::Text
                    };
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut result, &mut text, &mut markup, &kind, offset);
                offset = at;
                kind = ReadKind::Heading(level as u8);
            }
            Event::Start(Tag::CodeBlock(block)) => {
                flush(&mut result, &mut text, &mut markup, &kind, offset);
                kind = ReadKind::Code(match block {
                    pulldown_cmark::CodeBlockKind::Fenced(s) => s.into_string(),
                    _ => String::new(),
                });
                offset = at;
                code = true;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                quote = quote.saturating_sub(1);
            }
            Event::Start(Tag::List(start)) => list.push(start),
            Event::End(TagEnd::List(_)) => {
                list.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut result, &mut text, &mut markup, &kind, offset);
                offset = at;
                kind = ReadKind::Text;
                let bullet = match list.last_mut() {
                    Some(Some(n)) => {
                        let v = format!("{n}. ");
                        *n += 1;
                        v
                    }
                    _ => "• ".into(),
                };
                text.push_str(&bullet);
                markup.push_str(&escape(&bullet));
            }
            Event::TaskListMarker(checked) => {
                kind = ReadKind::Task {
                    checked,
                    offset: at + 1,
                };
                text.clear();
                markup.clear();
            }
            Event::Start(Tag::Emphasis) => markup.push_str("<i>"),
            Event::End(TagEnd::Emphasis) => markup.push_str("</i>"),
            Event::Start(Tag::Strong) => markup.push_str("<b>"),
            Event::End(TagEnd::Strong) => markup.push_str("</b>"),
            Event::Start(Tag::Strikethrough) => markup.push_str("<s>"),
            Event::End(TagEnd::Strikethrough) => markup.push_str("</s>"),
            Event::Start(Tag::Link {
                dest_url,
                link_type,
                ..
            }) => markup.push_str(&format!(
                "<a href=\"{}{}\">",
                if matches!(link_type, pulldown_cmark::LinkType::WikiLink { .. }) {
                    "notes-wiki:"
                } else {
                    "notes-md:"
                },
                escape(&dest_url)
            )),
            Event::End(TagEnd::Link) => markup.push_str("</a>"),
            Event::Start(Tag::Image {
                dest_url,
                link_type,
                ..
            }) => {
                let mut inline = Vec::new();
                for fragment in markup.split('<').skip(1) {
                    if let Some((tag, _)) = fragment.split_once('>') {
                        if tag.starts_with('/') {
                            inline.pop();
                        } else {
                            inline.push(tag.to_owned());
                        }
                    }
                }
                for tag in inline.iter().rev() {
                    markup.push_str(&format!("</{}>", tag.split_whitespace().next().unwrap()));
                }
                flush(&mut result, &mut text, &mut markup, &kind, offset);
                for tag in inline {
                    markup.push_str(&format!("<{tag}>"));
                }
                offset = range.end + base;
                result.push(ReadBlock {
                    offset: at,
                    kind: ReadKind::Embed {
                        target: dest_url.to_string(),
                        wiki: matches!(link_type, pulldown_cmark::LinkType::WikiLink { .. }),
                    },
                    markup: String::new(),
                    text: dest_url.to_string(),
                });
                image += 1;
            }
            Event::End(TagEnd::Image) => image = image.saturating_sub(1),
            Event::Start(Tag::Table(_)) => {
                flush(&mut result, &mut text, &mut markup, &kind, offset);
                offset = at;
                kind = ReadKind::Table;
            }
            Event::End(TagEnd::TableCell) => {
                text.push_str("  │  ");
                markup.push_str("  │  ");
            }
            Event::End(TagEnd::TableRow | TagEnd::TableHead) => {
                text.push('\n');
                markup.push('\n');
            }
            Event::Code(value) => {
                text.push_str(&value);
                markup.push_str(&format!("<tt>{}</tt>", escape(&value)));
            }
            Event::InlineMath(value) | Event::DisplayMath(value) => {
                text.push_str(&value);
                markup.push_str(&format!("<i>{}</i>", escape(&math_text(&value))));
            }
            Event::Text(value) if image == 0 => {
                text.push_str(&value);
                markup.push_str(&escape(&value));
            }
            Event::FootnoteReference(value) => {
                text.push_str(&format!("[{value}]"));
                markup.push_str(&format!("<sup>{}</sup>", escape(&value)));
            }
            Event::SoftBreak | Event::HardBreak => {
                text.push('\n');
                markup.push('\n');
            }
            Event::Rule => {
                flush(&mut result, &mut text, &mut markup, &kind, offset);
                result.push(ReadBlock {
                    offset: at,
                    kind: ReadKind::Rule,
                    markup: String::new(),
                    text: String::new(),
                });
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::CodeBlock
                | TagEnd::Table,
            ) => {
                flush(&mut result, &mut text, &mut markup, &kind, offset);
                kind = ReadKind::Text;
                code = false;
            }
            Event::Html(value) | Event::InlineHtml(value) => {
                text.push_str(&value);
                markup.push_str(&escape(&value));
            }
            _ => {}
        }
        let _ = code;
    }
    flush(&mut result, &mut text, &mut markup, &kind, offset);
    result
}
fn math_text(source: &str) -> String {
    let mut result = source.to_owned();
    for (from, to) in [
        ("\\alpha", "α"),
        ("\\beta", "β"),
        ("\\gamma", "γ"),
        ("\\delta", "δ"),
        ("\\theta", "θ"),
        ("\\pi", "π"),
        ("\\infty", "∞"),
        ("\\times", "×"),
        ("\\cdot", "·"),
        ("\\leq", "≤"),
        ("\\geq", "≥"),
        ("^2", "²"),
        ("^3", "³"),
    ] {
        result = result.replace(from, to);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_blocks() {
        let inline_image = blocks("**before ![[Embed]] after**");
        assert_eq!(inline_image[0].markup, "<b>before </b>");
        assert_eq!(inline_image[2].markup, "<b> after</b>");
        let blocks = blocks(
            "# Hello\n\n**bold** and [link](Note)\n\n- [ ] task\n\n![[Embed]]\n\n```rust\nfn main(){}\n```\n\n| A | B |\n| - | - |\n| x | y |\n",
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b.kind, ReadKind::Heading(1)))
        );
        assert!(blocks.iter().any(|b| b.markup.contains("<b>bold</b>")));
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b.kind, ReadKind::Task { .. }))
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b.kind, ReadKind::Embed { .. }))
        );
        assert!(blocks.iter().any(|b| matches!(b.kind, ReadKind::Code(_))));
        assert!(blocks.iter().any(|b| matches!(b.kind, ReadKind::Table)));
    }
}
