//! Source edits describe one undoable replacement; callers retain all unrelated bytes.
use std::ops::Range;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub replacement: String,
}
fn prefix(line: &str) -> Option<(usize, String, bool)> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let (marker, tail) =
        if rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") {
            (rest[..2].to_owned(), &rest[2..])
        } else {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 || digits > 9 {
                return None;
            }
            let after = &rest[digits..];
            if !after.starts_with(". ") && !after.starts_with(") ") {
                return None;
            }
            let number = rest[..digits].parse::<u64>().ok()?;
            (format!("{}{} ", number + 1, &after[..1]), &after[2..])
        };
    let task = tail.starts_with("[ ] ") || tail.starts_with("[x] ") || tail.starts_with("[X] ");
    let consumed = if task {
        line.len() - tail.len() + 4
    } else {
        line.len() - tail.len()
    };
    Some((
        consumed,
        format!(
            "{}{}{}",
            &line[..indent],
            marker,
            if task { "[ ] " } else { "" }
        ),
        line[consumed..].trim().is_empty(),
    ))
}
pub fn continue_list(source: &str, cursor: usize) -> Option<Edit> {
    if !source.is_char_boundary(cursor) {
        return None;
    }
    let start = source[..cursor].rfind('\n').map_or(0, |p| p + 1);
    let line = &source[start..cursor];
    if line.ends_with('\r') {
        return None;
    }
    let (consumed, next, empty) = prefix(line)?;
    // Do not interpret list-looking content inside fenced code or YAML.
    let mut fence: Option<(u8, usize)> = None;
    for previous in source[..start].lines() {
        let line = previous.trim_start_matches([' ', '\t', '>']);
        let count = line
            .bytes()
            .take_while(|b| *b == b'`' || *b == b'~')
            .count();
        if count < 3 {
            continue;
        }
        let marker = line.as_bytes()[0];
        match fence {
            Some((open, length))
                if marker == open && count >= length && line[count..].trim().is_empty() =>
            {
                fence = None
            }
            None => fence = Some((marker, count)),
            _ => {}
        }
    }
    if fence.is_some() {
        return None;
    }
    if source.lines().next() == Some("---") {
        let mut end = 0;
        let mut closed = None;
        for (index, line) in source.split_inclusive('\n').enumerate() {
            end += line.len();
            if index > 0 && matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
                closed = Some(end);
                break;
            }
        }
        if closed.is_some_and(|end| cursor < end) {
            return None;
        }
    }
    if empty {
        Some(Edit {
            range: start..start + consumed,
            replacement: String::new(),
        })
    } else {
        let ending = if source[cursor..].starts_with("\r\n") || source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        Some(Edit {
            range: cursor..cursor,
            replacement: format!("{ending}{next}"),
        })
    }
}
pub fn toggle_task(source: &str, cursor: usize) -> Option<Edit> {
    let parsed = crate::parse(source);
    parsed
        .tasks
        .iter()
        .find(|t| t.line.contains(&cursor) || cursor == t.line.end)
        .map(|task| Edit {
            range: task.marker.clone(),
            replacement: if task.checked { " ".into() } else { "x".into() },
        })
}
fn comment_at(line: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            }
        }
        if ch == '#' && quote.is_none() && line[..index].ends_with(' ') {
            return Some(index);
        }
    }
    None
}
/// Exact editable YAML value, rather than the bounded Inspector display string.
pub fn property_value(source: &str, key: &str) -> Option<String> {
    let range = crate::metadata::frontmatter(source).range?;
    let mut lines = source[..range.end].split_inclusive('\n').skip(1);
    while let Some(line) = lines.next() {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        let first = comment_at(value).map_or(value, |i| &value[..i]);
        let mut result = first
            .trim_start_matches(' ')
            .trim_end_matches([' ', '\r', '\n'])
            .to_owned();
        for next in lines {
            if !next.starts_with([' ', '\t']) && !next.trim().is_empty() {
                break;
            }
            result.push('\n');
            result.push_str(next.trim_end_matches(['\r', '\n']));
        }
        return Some(result);
    }
    None
}
pub fn set_property(source: &str, key: &str, value: Option<&str>) -> Result<Edit, String> {
    let key = key.trim();
    if key.is_empty() || key.contains(['\n', '\r', ':', '[', ']', '{', '}', '#', '\0']) {
        return Err("Use a plain YAML property name".into());
    }
    if let Some(value) = value {
        use saphyr::{LoadableYamlNode, Yaml};
        let docs = Yaml::load_from_str(&format!("{key}: {value}\n")).map_err(|e| e.to_string())?;
        if docs.len() != 1
            || docs[0].as_mapping().is_none_or(|m| m.len() != 1)
            || docs[0].as_mapping_get(key).is_none()
        {
            return Err("Enter a valid YAML value".into());
        }
    }
    let metadata = crate::metadata::frontmatter(source);
    if metadata.error.is_some() {
        return Err("Repair the YAML frontmatter before editing properties".into());
    }
    let ending = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    if let Some(range) = metadata.range {
        let lines = source[..range.end]
            .split_inclusive('\n')
            .collect::<Vec<_>>();
        for (i, line) in lines.iter().enumerate().skip(1) {
            let offset: usize = lines[..i].iter().map(|l| l.len()).sum();
            let Some((name, _)) = line.split_once(':') else {
                continue;
            };
            if name.trim() == key && !line.starts_with([' ', '\t']) {
                let mut end = offset + line.len();
                for next in lines.iter().skip(i + 1) {
                    if next.starts_with([' ', '\t']) || next.trim().is_empty() {
                        end += next.len();
                    } else {
                        break;
                    }
                }
                // Preserve a trailing comment on the property line when replacing its value.
                let comment = comment_at(line)
                    .map(|i| format!(" {}", line[i..].trim_end_matches(['\r', '\n'])))
                    .unwrap_or_default();
                return Ok(Edit {
                    range: offset..end,
                    replacement: value
                        .map(|v| {
                            let normalized = v.replace("\r\n", "\n");
                            let mut lines = normalized.split('\n');
                            let first = lines.next().unwrap_or("");
                            let mut output = format!("{key}: {first}{comment}{ending}");
                            for line in lines {
                                output.push_str(line);
                                output.push_str(ending);
                            }
                            output
                        })
                        .unwrap_or_default(),
                });
            }
        }
        if metadata.properties.iter().any(|(name, _)| name == key) {
            return Err("Edit this property's complex YAML key in source mode".into());
        }
        let close = source[..range.end].rfind('\n').map_or(0, |p| {
            if p + 1 == range.end {
                source[..p].rfind('\n').map_or(0, |n| n + 1)
            } else {
                p + 1
            }
        });
        Ok(Edit {
            range: close..close,
            replacement: value
                .map(|v| format!("{key}: {v}{ending}"))
                .unwrap_or_default(),
        })
    } else {
        Ok(Edit {
            range: 0..0,
            replacement: value
                .map(|v| format!("---{ending}{key}: {v}{ending}---{ending}"))
                .unwrap_or_default(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_edits() {
        assert_eq!(
            continue_list("- [x] Café", 11).unwrap().replacement,
            "\n- [ ] "
        );
        assert_eq!(continue_list("9. hello", 8).unwrap().replacement, "\n10. ");
        assert_eq!(continue_list("- ", 2).unwrap().replacement, "");
        assert!(continue_list("```\n- text\n```", 10).is_none());
        let source = "---\r\ntitle: Hello # keep\r\nother: [a, b]\r\n---\r\nBody [[Note]]";
        let edit = set_property(source, "title", Some("Changed")).unwrap();
        let mut text = source.to_owned();
        text.replace_range(edit.range, &edit.replacement);
        assert_eq!(
            text,
            "---\r\ntitle: Changed # keep\r\nother: [a, b]\r\n---\r\nBody [[Note]]"
        );
        assert!(set_property("---\nx: [bad\n---\n", "x", Some("a")).is_err());
        let complex = "---\ncode: '001' # keep\ntags:\n  - alpha\n  - beta\n---\nBody";
        assert_eq!(property_value(complex, "code").as_deref(), Some("'001'"));
        assert_eq!(
            property_value(complex, "tags").as_deref(),
            Some("\n  - alpha\n  - beta")
        );
        assert!(set_property(complex, "code", Some("x\nother: replaced")).is_err());
        let literal = "---\ndescription: | # keep\n  first\n\n  last\nstatus: draft\n---\nBody";
        let value = property_value(literal, "description").unwrap();
        assert_eq!(value, "|\n  first\n\n  last");
        let edit = set_property(literal, "description", Some(&value)).unwrap();
        let mut roundtrip = literal.to_owned();
        roundtrip.replace_range(edit.range, &edit.replacement);
        assert_eq!(roundtrip, literal);
        assert!(set_property("---\n'quoted': x\n---\n", "quoted", Some("y")).is_err());
    }
}
