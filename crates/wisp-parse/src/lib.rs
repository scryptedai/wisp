//! `wisp-parse`: lossless frontmatter CST-lite, markdown sectioning, anchors, links, footnotes (§7).

pub mod frontmatter;
pub mod sections;

pub use frontmatter::{FmItem, Frontmatter};
pub use sections::{Anchor, LinkForm, LinkRef, ParsedSection, Sectioned};

/// A whole parsed concept file.
#[derive(Clone, Debug)]
pub struct ParsedFile {
    /// `"\n"` or `"\r\n"`.
    pub line_ending: &'static str,
    /// None when the file has no parseable frontmatter block (non-conformant).
    pub frontmatter: Option<Frontmatter>,
    pub sectioned: Sectioned,
}

impl ParsedFile {
    pub fn sections(&self) -> &[ParsedSection] {
        &self.sectioned.sections
    }
    /// The `wisp_id` entry value, if present and well-formed.
    pub fn wisp_id(&self, key: &str) -> Option<wisp_model::ConceptId> {
        let raw = self.frontmatter.as_ref()?.get(key)?;
        let v = frontmatter::entry_string(raw)?;
        v.trim().parse().ok()
    }
    /// The `type` entry value as a string, if any.
    pub fn type_value(&self) -> Option<String> {
        self.frontmatter
            .as_ref()?
            .get("type")
            .and_then(frontmatter::entry_string)
    }
}

/// Parse a concept file. Non-UTF-8 input must be rejected by the caller with a clear error.
pub fn parse_file(text: &str) -> ParsedFile {
    let line_ending: &'static str = if text.contains("\r\n") { "\r\n" } else { "\n" };
    match frontmatter::split(text) {
        Some((fm, body)) => ParsedFile {
            line_ending,
            frontmatter: Some(fm),
            sectioned: sections::section(body),
        },
        None => ParsedFile {
            line_ending,
            frontmatter: None,
            sectioned: sections::section(text),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_file() {
        let src = "---\ntype: Table\nwisp_id: 01JAB7Q3M8N5Z6K2P9R0000000\n---\n<!-- ^01JAB7Q3M8N5Z6K2P9R0000001 -->\nPreamble.\n\n## Schema <!-- ^01JAB7Q3M8N5Z6K2P9R0000002 -->\ncolumns\n";
        let f = parse_file(src);
        assert!(f.wisp_id("wisp_id").is_some());
        assert_eq!(f.type_value().as_deref(), Some("Table"));
        assert_eq!(f.sections().len(), 2);
        assert_eq!(f.sections()[0].body, "Preamble.\n\n");
        assert_eq!(f.sections()[1].heading, "Schema");
    }
}
