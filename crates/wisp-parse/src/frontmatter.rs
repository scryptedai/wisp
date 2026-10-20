//! Lossless frontmatter entries (§7.2).
//!
//! The frontmatter is the text between the opening `---` line and the next `---` line. Each
//! top-level mapping entry is stored as its key name plus its raw span bytes (from the first
//! comment line directly above it through the end of its value, including the trailing line
//! break). Bytes between entries are kept as separator spans. Untouched entries are emitted
//! as the original bytes, so an untouched concept reproduces its frontmatter exactly.
//!
//! See DECISIONS.md §7.2: this build splits entries with a line-structured scanner instead of a
//! tree-sitter CST. Values are parsed with `serde_yaml` only for comparison.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FmItem {
    /// One top-level entry: the key and its raw span.
    Entry { key: String, raw: String },
    /// Blank lines and free-standing comments between entries, verbatim.
    Separator(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Frontmatter {
    /// The opening delimiter line including its line break (`---\n`).
    pub opening: String,
    pub items: Vec<FmItem>,
    /// The closing delimiter line including its line break.
    pub closing: String,
    /// True when the top level is not a block mapping; the whole frontmatter is one opaque span.
    pub opaque: bool,
}

impl Frontmatter {
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.items.iter().filter_map(|i| match i {
            FmItem::Entry { key, raw } => Some((key.as_str(), raw.as_str())),
            _ => None,
        })
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries().find(|(k, _)| *k == key).map(|(_, r)| r)
    }
    /// Render the frontmatter body (between the delimiters).
    pub fn render_body(&self) -> String {
        let mut s = String::new();
        for i in &self.items {
            match i {
                FmItem::Entry { raw, .. } => s.push_str(raw),
                FmItem::Separator(raw) => s.push_str(raw),
            }
        }
        s
    }
    pub fn render(&self) -> String {
        format!("{}{}{}", self.opening, self.render_body(), self.closing)
    }
}

/// Split a file into (frontmatter, body). The body starts right after the closing `---` line.
/// Returns `None` when the file has no parseable frontmatter block.
pub fn split(text: &str) -> Option<(Frontmatter, &str)> {
    let text_nobom = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bom_len = text.len() - text_nobom.len();
    let first_line_end = text_nobom.find('\n')?;
    let first_line = &text_nobom[..=first_line_end];
    if first_line.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }
    let after_open = first_line_end + 1;
    let rest = &text_nobom[after_open..];
    // Find the next line that is exactly "---".
    let mut pos = 0usize;
    let mut close: Option<(usize, usize)> = None; // (start, end incl. newline)
    while pos <= rest.len() {
        let line_end = rest[pos..]
            .find('\n')
            .map(|i| pos + i + 1)
            .unwrap_or(rest.len());
        let line = &rest[pos..line_end];
        if line.trim_end_matches(['\r', '\n']) == "---" {
            close = Some((pos, line_end));
            break;
        }
        if line_end == rest.len() {
            break;
        }
        pos = line_end;
    }
    let (cs, ce) = close?;
    let fm_text = &rest[..cs];
    let closing = rest[cs..ce].to_string();
    let body = &rest[ce..];
    let mut fm = parse_body(fm_text);
    fm.opening = format!("{}{}", &text[..bom_len], first_line);
    fm.closing = closing;
    Some((fm, body))
}

fn line_iter(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < s.len() {
        let end = s[pos..].find('\n').map(|i| pos + i + 1).unwrap_or(s.len());
        out.push(&s[pos..end]);
        pos = end;
    }
    out
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#') && !line.starts_with(' ') && !line.starts_with('\t')
}

/// A line that begins a top-level mapping entry: `key:` at column 0.
fn entry_key(line: &str) -> Option<String> {
    let l = line.trim_end_matches(['\r', '\n']);
    let first = l.chars().next()?;
    if first.is_whitespace() || first == '#' || first == '-' {
        return None;
    }
    // Quoted keys.
    if first == '"' || first == '\'' {
        let close = l[1..].find(first)? + 1;
        let after = &l[close + 1..];
        if after.starts_with(':')
            && (after.len() == 1 || after[1..].starts_with(' ') || after[1..].starts_with('\t'))
        {
            return Some(l[1..close].to_string());
        }
        return None;
    }
    // Plain key: up to the first ": " or ":" at end of line. Keys may not contain '#'.
    let mut idx = 0;
    let bytes = l.as_bytes();
    while idx < bytes.len() {
        if bytes[idx] == b':' {
            let at_end = idx + 1 == bytes.len();
            let followed_by_space = !at_end && (bytes[idx + 1] == b' ' || bytes[idx + 1] == b'\t');
            if at_end || followed_by_space {
                let key = l[..idx].trim_end();
                if key.is_empty() || key.contains('#') {
                    return None;
                }
                return Some(key.to_string());
            }
        }
        idx += 1;
    }
    None
}

/// Parse the text between the delimiters into entries and separators.
pub fn parse_body(fm_text: &str) -> Frontmatter {
    let lines = line_iter(fm_text);
    let mut items: Vec<FmItem> = Vec::new();
    // Pending comment lines directly above a possible entry.
    let mut pending_comments = String::new();
    // Pending blank lines after an entry (attached to the entry only if a continuation follows).
    let mut pending_blanks = String::new();
    let mut cur: Option<(String, String)> = None; // (key, raw)
    let mut any_entry = false;
    let mut nonconforming = false;

    let flush_cur = |cur: &mut Option<(String, String)>, items: &mut Vec<FmItem>| {
        if let Some((k, r)) = cur.take() {
            items.push(FmItem::Entry { key: k, raw: r });
        }
    };

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if is_blank(line) {
            if cur.is_some() {
                pending_blanks.push_str(line);
            } else {
                if !pending_comments.is_empty() {
                    // A comment block followed by a blank line is free-standing.
                    items.push(FmItem::Separator(std::mem::take(&mut pending_comments)));
                }
                items.push(FmItem::Separator(line.to_string()));
            }
            i += 1;
            continue;
        }
        if let Some(key) = entry_key(line) {
            // A new entry starts. Close the current one; pending blanks become a separator.
            flush_cur(&mut cur, &mut items);
            if !pending_blanks.is_empty() {
                items.push(FmItem::Separator(std::mem::take(&mut pending_blanks)));
            }
            let mut raw = std::mem::take(&mut pending_comments);
            raw.push_str(line);
            cur = Some((key, raw));
            any_entry = true;
            i += 1;
            continue;
        }
        if is_comment(line) && cur.is_none() {
            pending_comments.push_str(line);
            i += 1;
            continue;
        }
        // Continuation of the current entry (indented lines, `- ` sequence items, multi-line
        // flow collections, block scalars, or indented comments).
        if let Some((_, raw)) = cur.as_mut() {
            if is_comment(line) {
                // A column-0 comment after an entry: it belongs to the NEXT entry if one follows
                // directly, otherwise it is free-standing. Look ahead.
                let mut j = i;
                let mut block = String::new();
                while j < lines.len() && is_comment(lines[j]) {
                    block.push_str(lines[j]);
                    j += 1;
                }
                if j < lines.len() && entry_key(lines[j]).is_some() && pending_blanks.is_empty() {
                    // Comments directly above the next entry.
                    flush_cur(&mut cur, &mut items);
                    pending_comments.push_str(&block);
                } else {
                    flush_cur(&mut cur, &mut items);
                    if !pending_blanks.is_empty() {
                        items.push(FmItem::Separator(std::mem::take(&mut pending_blanks)));
                    }
                    items.push(FmItem::Separator(block));
                }
                i = j;
                continue;
            }
            raw.push_str(&std::mem::take(&mut pending_blanks));
            raw.push_str(line);
            i += 1;
            continue;
        }
        // A non-entry, non-comment line with no current entry: not a block mapping.
        nonconforming = true;
        break;
    }
    flush_cur(&mut cur, &mut items);
    if !pending_blanks.is_empty() {
        items.push(FmItem::Separator(pending_blanks));
    }
    if !pending_comments.is_empty() {
        items.push(FmItem::Separator(pending_comments));
    }

    if nonconforming || (!any_entry && !fm_text.trim().is_empty()) {
        return Frontmatter {
            opening: String::new(),
            items: vec![FmItem::Separator(fm_text.to_string())],
            closing: String::new(),
            opaque: true,
        };
    }
    Frontmatter {
        opening: String::new(),
        items,
        closing: String::new(),
        opaque: false,
    }
}

/// Parse an entry's value (for comparison and managed keys). Comment lines above the key are ignored.
pub fn entry_value(raw: &str) -> Option<serde_yaml::Value> {
    let doc = strip_leading_comments(raw);
    let v: serde_yaml::Value = serde_yaml::from_str(doc).ok()?;
    match v {
        serde_yaml::Value::Mapping(m) => m.into_iter().next().map(|(_, v)| v),
        _ => None,
    }
}

fn strip_leading_comments(raw: &str) -> &str {
    let mut rest = raw;
    loop {
        let line_end = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
        let line = &rest[..line_end];
        if is_comment(line) {
            rest = &rest[line_end..];
            if rest.is_empty() {
                break;
            }
        } else {
            break;
        }
    }
    rest
}

/// The scalar string value of an entry when it is a plain string, else None.
pub fn entry_string(raw: &str) -> Option<String> {
    match entry_value(raw)? {
        serde_yaml::Value::String(s) => Some(s),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Build the raw span for a new entry from a key and a YAML value.
pub fn format_entry(key: &str, value: &serde_yaml::Value, line_ending: &str) -> String {
    let mut m = serde_yaml::Mapping::new();
    m.insert(serde_yaml::Value::String(key.to_string()), value.clone());
    let s = serde_yaml::to_string(&serde_yaml::Value::Mapping(m)).unwrap_or_default();
    // serde_yaml emits "\n" line endings; normalize to the file's.
    let s = s.trim_end_matches('\n');
    let mut out = String::new();
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push_str(line_ending);
        }
        out.push_str(line);
    }
    out.push_str(line_ending);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_roundtrips() {
        let src = "---\n# leading comment\ntype: BigQuery Table\ntitle: \"Customer Orders\"\n\ntags:\n  - sales\n  - orders   # inline\n# free comment\n\ndescription: >\n  folded\n  text\n---\n# Heading\nbody\n";
        let (fm, body) = split(src).unwrap();
        assert!(!fm.opaque);
        assert_eq!(body, "# Heading\nbody\n");
        assert_eq!(fm.render(), &src[..src.find("# Heading").unwrap()]);
        let keys: Vec<_> = fm.entries().map(|(k, _)| k.to_string()).collect();
        assert_eq!(keys, vec!["type", "title", "tags", "description"]);
        assert_eq!(
            fm.get("type").unwrap(),
            "# leading comment\ntype: BigQuery Table\n"
        );
        assert_eq!(
            entry_string(fm.get("title").unwrap()).unwrap(),
            "Customer Orders"
        );
        let tags = entry_value(fm.get("tags").unwrap()).unwrap();
        assert!(matches!(tags, serde_yaml::Value::Sequence(ref s) if s.len() == 2));
    }

    #[test]
    fn crlf_and_flow() {
        let src = "---\r\ntype: X\r\ntags: [a,\r\n  b]\r\n---\r\nbody\r\n";
        let (fm, body) = split(src).unwrap();
        assert_eq!(body, "body\r\n");
        assert_eq!(fm.get("tags").unwrap(), "tags: [a,\r\n  b]\r\n");
        assert_eq!(
            fm.render(),
            "---\r\ntype: X\r\ntags: [a,\r\n  b]\r\n---\r\n"
        );
    }

    #[test]
    fn non_mapping_is_opaque() {
        let src = "---\n[flow, mapping]\n---\nbody";
        let (fm, _) = split(src).unwrap();
        assert!(fm.opaque);
        assert_eq!(fm.render_body(), "[flow, mapping]\n");
    }

    #[test]
    fn no_frontmatter() {
        assert!(split("# just markdown\n").is_none());
        assert!(split("---\nnever closed\n").is_none());
    }

    #[test]
    fn format_entry_simple() {
        let v = serde_yaml::Value::String("draft".into());
        assert_eq!(format_entry("status", &v, "\n"), "status: draft\n");
    }
}
