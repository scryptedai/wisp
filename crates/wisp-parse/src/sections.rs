//! Markdown sectioning, anchors, links and footnotes (§7.3).

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use wisp_model::SectionId;

/// The 26-character Crockford base32 alphabet.
const CROCKFORD: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    Valid(SectionId),
    /// Well-formed shape but not a decodable ULID; treated as unanchored (§7.5 rule 5).
    Malformed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkForm {
    Absolute,
    Relative,
    External,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRef {
    pub dest: String,
    pub form: LinkForm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSection {
    pub anchor: Option<Anchor>,
    /// 0 for the preamble.
    pub level: u8,
    pub heading: String,
    /// Heading line(s) as written, anchor removed, including the trailing line break. Empty for the preamble.
    pub heading_line: String,
    /// Body bytes verbatim (erasure markers removed; preamble anchor line removed).
    pub body: String,
    pub links: Vec<LinkRef>,
    pub footnotes: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Sectioned {
    pub sections: Vec<ParsedSection>,
    pub warnings: Vec<String>,
    /// Erasure markers found in the file (section IDs), which `record` ignores.
    pub erasure_markers: Vec<SectionId>,
}

/// Format an anchor comment for a section (§7.3).
pub fn anchor_text(id: SectionId) -> String {
    format!("<!-- ^{id} -->")
}

/// Format an erasure marker (§7.3).
pub fn erasure_marker(id: SectionId, reason: &str) -> String {
    format!("<!-- ^{id} [erased: {reason}] -->")
}

/// Try to parse `<!-- ^XXXXXXXXXXXXXXXXXXXXXXXXXX -->` exactly.
pub fn parse_anchor(s: &str) -> Option<Anchor> {
    let inner = s.strip_prefix("<!-- ^")?.strip_suffix(" -->")?;
    if inner.len() != 26 {
        return None;
    }
    if !inner
        .chars()
        .all(|c| CROCKFORD.contains(c.to_ascii_uppercase()))
    {
        return Some(Anchor::Malformed(inner.to_string()));
    }
    match inner.parse::<SectionId>() {
        Ok(id) => Some(Anchor::Valid(id)),
        Err(_) => Some(Anchor::Malformed(inner.to_string())),
    }
}

/// Parse an erasure marker line (without line ending): `<!-- ^ID [erased: reason] -->`.
pub fn parse_erasure_marker(line: &str) -> Option<(SectionId, String)> {
    let inner = line.trim().strip_prefix("<!-- ^")?.strip_suffix(" -->")?;
    let (id, rest) = inner.split_once(' ')?;
    let reason = rest.strip_prefix("[erased: ")?.strip_suffix(']')?;
    let id = id.parse::<SectionId>().ok()?;
    Some((id, reason.to_string()))
}

fn line_ending_of(s: &str) -> &'static str {
    if s.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Split a heading's first line into (text-without-anchor, anchor).
/// The anchor is the last element on the line, separated by exactly one space (§7.3).
fn take_trailing_anchor(line: &str) -> (String, Option<Anchor>) {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if let Some(idx) = trimmed.rfind("<!-- ^") {
        let candidate = &trimmed[idx..];
        if let Some(a) = parse_anchor(candidate) {
            let mut head = &trimmed[..idx];
            if head.ends_with(' ') {
                head = &head[..head.len() - 1];
            }
            return (head.to_string(), Some(a));
        }
    }
    (trimmed.to_string(), None)
}

fn atx_heading_text(line: &str) -> String {
    let l = line.trim();
    let l = l.trim_start_matches('#').trim_start();
    // Optional closing sequence of #s preceded by a space.
    let mut t = l.trim_end();
    if let Some(stripped) = t.trim_end_matches('#').strip_suffix(' ') {
        t = stripped.trim_end();
    } else if t.chars().all(|c| c == '#') {
        t = "";
    }
    t.to_string()
}

fn classify_link(dest: &str) -> LinkForm {
    if dest.starts_with('#') {
        return LinkForm::External;
    }
    let mut chars = dest.chars();
    if let Some(c0) = chars.next() {
        if c0.is_ascii_alphabetic() {
            let scheme_end = dest.find(':');
            if let Some(e) = scheme_end {
                if dest[..e]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
                {
                    return LinkForm::External;
                }
            }
        }
    }
    if dest.starts_with('/') {
        LinkForm::Absolute
    } else {
        LinkForm::Relative
    }
}

/// Section the body of a concept. `body` starts right after the frontmatter's closing `---` line.
pub fn section(body: &str) -> Sectioned {
    let mut out = Sectioned::default();
    let le = line_ending_of(body);

    // 1. Remove erasure markers (whole lines) before parsing; record ignores them.
    let mut cleaned = String::with_capacity(body.len());
    {
        let mut pos = 0;
        while pos < body.len() {
            let end = body[pos..]
                .find('\n')
                .map(|i| pos + i + 1)
                .unwrap_or(body.len());
            let line = &body[pos..end];
            if let Some((id, _)) = parse_erasure_marker(line) {
                out.erasure_markers.push(id);
            } else {
                cleaned.push_str(line);
            }
            pos = end;
        }
    }
    let text = cleaned.as_str();

    // 2. Find headings with offsets. Headings inside block quotes are not sections.
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    let parser = Parser::new_ext(text, opts);
    let mut bq_depth = 0usize;
    let mut headings: Vec<(usize, usize, u8)> = Vec::new(); // (start, end, level)
    let mut links: Vec<(usize, LinkRef)> = Vec::new();
    let mut footnotes: Vec<(usize, String)> = Vec::new();
    let mut in_code = 0usize;
    for (ev, range) in parser.into_offset_iter() {
        match ev {
            Event::Start(Tag::BlockQuote(_)) => bq_depth += 1,
            Event::End(TagEnd::BlockQuote(_)) => bq_depth = bq_depth.saturating_sub(1),
            Event::Start(Tag::CodeBlock(_)) => in_code += 1,
            Event::End(TagEnd::CodeBlock) => in_code = in_code.saturating_sub(1),
            Event::Start(Tag::Heading { level, .. }) => {
                if bq_depth == 0 && in_code == 0 {
                    headings.push((range.start, range.end, level as u8));
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let dest = dest_url.to_string();
                let form = classify_link(&dest);
                links.push((range.start, LinkRef { dest, form }));
            }
            Event::FootnoteReference(label) => footnotes.push((range.start, label.to_string())),
            Event::Html(h) | Event::InlineHtml(h) => {
                // Anchor-shaped comments outside a heading line carry no identity (§7.3).
                let s = h.trim();
                if s.starts_with("<!-- ^") && parse_anchor(s).is_some() {
                    // Decided below once section boundaries are known: warn if not a heading anchor
                    // (heading anchors are consumed from the heading line, not emitted as HTML).
                    out.warnings.push(format!(
                        "anchor-shaped comment at byte {} is ordinary text and carries no identity",
                        range.start
                    ));
                }
            }
            _ => {}
        }
    }

    // 3. Compute section boundaries. Each heading section runs from its heading start to the next heading start.
    let mut boundaries: Vec<(usize, usize, u8)> = Vec::new(); // (section_start, heading_end, level)
    for (i, (hs, he, lvl)) in headings.iter().enumerate() {
        let _ = i;
        // Extend heading end to the end of its last line, including the line break.
        let mut end = *he;
        if end > 0 && !text[..end].ends_with('\n') {
            end = text[end..]
                .find('\n')
                .map(|k| end + k + 1)
                .unwrap_or(text.len());
        }
        boundaries.push((*hs, end, *lvl));
    }

    let first_heading = boundaries.first().map(|b| b.0).unwrap_or(text.len());

    // Preamble.
    {
        let mut pre = &text[..first_heading];
        let mut anchor = None;
        // Own-line anchor immediately after the frontmatter.
        let first_line_end = pre.find('\n').map(|i| i + 1).unwrap_or(pre.len());
        let first_line = &pre[..first_line_end];
        let fl = first_line.trim_end_matches(['\r', '\n']);
        if fl.starts_with("<!-- ^") {
            if let Some(a) = parse_anchor(fl) {
                anchor = Some(a);
                pre = &pre[first_line_end..];
                // That anchor was consumed; drop the matching warning.
                out.warnings.retain(|w| !w.contains("at byte 0 "));
            }
        }
        let (ls, fns) = collect_in_range(&links, &footnotes, 0, first_heading);
        out.sections.push(ParsedSection {
            anchor,
            level: 0,
            heading: String::new(),
            heading_line: String::new(),
            body: pre.to_string(),
            links: ls,
            footnotes: fns,
        });
    }

    for (i, (hs, he, lvl)) in boundaries.iter().enumerate() {
        let next = boundaries.get(i + 1).map(|b| b.0).unwrap_or(text.len());
        let heading_raw = &text[*hs..*he];
        let body = &text[*he..next];
        // The anchor is on the first line of the heading block.
        let first_line_end = heading_raw
            .find('\n')
            .map(|k| k + 1)
            .unwrap_or(heading_raw.len());
        let first_line = &heading_raw[..first_line_end];
        let rest_lines = &heading_raw[first_line_end..];
        let (first_no_anchor, anchor) = take_trailing_anchor(first_line);
        let ending = if first_line.ends_with("\r\n") {
            "\r\n"
        } else if first_line.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let heading_line = format!("{first_no_anchor}{ending}{rest_lines}");
        let is_atx = first_no_anchor.trim_start().starts_with('#');
        let heading = if is_atx {
            atx_heading_text(&first_no_anchor)
        } else {
            first_no_anchor.trim().to_string()
        };
        // Drop the warning for this heading's anchor, if any was emitted for it.
        if anchor.is_some() {
            out.warnings.retain(|w| {
                !w.split("at byte ")
                    .nth(1)
                    .and_then(|s| s.split(' ').next())
                    .and_then(|n| n.parse::<usize>().ok())
                    .map(|n| n >= *hs && n < *he)
                    .unwrap_or(false)
            });
        }
        let (ls, fns) = collect_in_range(&links, &footnotes, *hs, next);
        out.sections.push(ParsedSection {
            anchor,
            level: *lvl,
            heading,
            heading_line,
            body: body.to_string(),
            links: ls,
            footnotes: fns,
        });
    }
    let _ = le;
    out
}

fn collect_in_range(
    links: &[(usize, LinkRef)],
    footnotes: &[(usize, String)],
    start: usize,
    end: usize,
) -> (Vec<LinkRef>, Vec<String>) {
    let ls = links
        .iter()
        .filter(|(p, _)| *p >= start && *p < end)
        .map(|(_, l)| l.clone())
        .collect();
    let fns = footnotes
        .iter()
        .filter(|(p, _)| *p >= start && *p < end)
        .map(|(_, f)| f.clone())
        .collect();
    (ls, fns)
}

/// Render sections back to a body. `ids[i]` is the anchor to write for section `i` (None = no anchor).
/// `erased_after[i]` are erasure markers to emit after section `i` (in order), each as (id, reason).
pub fn render_body(
    sections: &[(Option<SectionId>, u8, &str, &str)], // (anchor, level, heading_line, body)
    line_ending: &str,
    erased_markers: &dyn Fn(usize) -> Vec<(SectionId, String)>,
) -> String {
    let mut out = String::new();
    for (i, (anchor, level, heading_line, body)) in sections.iter().enumerate() {
        if *level == 0 {
            if let Some(id) = anchor {
                out.push_str(&anchor_text(*id));
                out.push_str(line_ending);
            }
            out.push_str(body);
        } else {
            match anchor {
                Some(id) => {
                    // Anchor goes at the end of the first heading line, after one space.
                    let first_end = heading_line
                        .find('\n')
                        .map(|k| k + 1)
                        .unwrap_or(heading_line.len());
                    let first = &heading_line[..first_end];
                    let rest = &heading_line[first_end..];
                    let trimmed = first.trim_end_matches(['\r', '\n']);
                    let ending = &first[trimmed.len()..];
                    out.push_str(trimmed);
                    out.push(' ');
                    out.push_str(&anchor_text(*id));
                    out.push_str(ending);
                    out.push_str(rest);
                }
                None => out.push_str(heading_line),
            }
            out.push_str(body);
        }
        for (id, reason) in erased_markers(i) {
            out.push_str(&erasure_marker(id, &reason));
            out.push_str(line_ending);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_atx_setext_fences_quotes() {
        let body = "intro text\n\n# One <!-- ^01JAB7Q3M8N5Z6K2P9R0000000 -->\nbody one\n\n```\n# not a heading\n```\n\n> # quoted not a heading\n\nTwo\n===\nbody two [link](../other.md) and [ext](https://x.y)\n\n### Three ###\n";
        let s = section(body);
        assert_eq!(s.sections.len(), 4);
        assert_eq!(s.sections[0].level, 0);
        assert_eq!(s.sections[0].body, "intro text\n\n");
        assert_eq!(s.sections[1].heading, "One");
        assert!(matches!(s.sections[1].anchor, Some(Anchor::Valid(_))));
        assert_eq!(s.sections[1].heading_line, "# One\n");
        assert!(s.sections[1].body.contains("# not a heading"));
        assert!(s.sections[1].body.contains("quoted"));
        assert_eq!(s.sections[2].heading, "Two");
        assert_eq!(s.sections[2].heading_line, "Two\n===\n");
        assert_eq!(s.sections[2].links.len(), 2);
        assert_eq!(s.sections[2].links[0].form, LinkForm::Relative);
        assert_eq!(s.sections[2].links[1].form, LinkForm::External);
        assert_eq!(s.sections[3].heading, "Three");
        assert_eq!(s.sections[3].heading_line, "### Three ###\n");
    }

    #[test]
    fn roundtrip_with_anchors() {
        let id1 = SectionId::new();
        let id2 = SectionId::new();
        let body = format!("<!-- ^{id1} -->\npreamble\n\n## A <!-- ^{id2} -->\nbody a\n");
        let s = section(&body);
        assert_eq!(s.sections.len(), 2);
        assert_eq!(s.sections[0].anchor, Some(Anchor::Valid(id1)));
        assert_eq!(s.sections[0].body, "preamble\n\n");
        assert_eq!(s.sections[1].anchor, Some(Anchor::Valid(id2)));
        let rendered = render_body(
            &[
                (Some(id1), 0, "", &s.sections[0].body),
                (
                    Some(id2),
                    2,
                    &s.sections[1].heading_line,
                    &s.sections[1].body,
                ),
            ],
            "\n",
            &|_| vec![],
        );
        assert_eq!(rendered, body);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    }

    #[test]
    fn erasure_markers_are_stripped() {
        let id = SectionId::new();
        let body = format!("## A\ntext\n{}\n## B\nmore\n", erasure_marker(id, "policy"));
        let s = section(&body);
        assert_eq!(s.erasure_markers, vec![id]);
        assert_eq!(s.sections[1].body, "text\n");
        assert_eq!(
            parse_erasure_marker(&erasure_marker(id, "policy"))
                .unwrap()
                .1,
            "policy"
        );
    }

    #[test]
    fn stray_anchor_warns() {
        let id = SectionId::new();
        let body = format!("## A\n\n<!-- ^{id} -->\n\ntext\n");
        let s = section(&body);
        assert_eq!(s.sections[1].anchor, None);
        assert_eq!(s.warnings.len(), 1);
    }
}
