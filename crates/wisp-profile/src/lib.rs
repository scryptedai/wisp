//! `wisp-profile`: a profile teaches the engine one OKF version (§12).
//! The core never changes when the spec does; a new spec version means a new profile.

use serde::{Deserialize, Serialize};
use wisp_model::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Must,
    Should,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub level: Level,
    pub code: String,
    pub message: String,
    pub path: String,
}

/// What a profile needs to see of a concept to validate or render it.
#[derive(Clone, Debug, Default)]
pub struct ConceptView {
    pub path: String,
    pub wisp_id: Option<ConceptId>,
    pub type_: Option<String>,
    /// User frontmatter entries (key, raw span), managed keys excluded.
    pub entries: Vec<(String, String)>,
    pub sections: Vec<(u8, String, String)>, // (level, heading, body)
    pub footnotes: Vec<String>,
    pub source_ids: Vec<String>,
    pub sources: Vec<serde_json::Value>,
    pub status: Option<Status>,
    /// (actor string, at) of the most recent content change.
    pub generated: Option<(String, String)>,
    /// (actor string, at), oldest first, at the current content version.
    pub verified: Vec<(String, String)>,
    /// Non-empty when retracted and rendered with --include-retracted: (at, by, reason).
    pub retracted: Option<(String, String, String)>,
    pub line_ending: String,
}

/// A directory entry for index/log rendering.
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub path: String, // concept path relative to the bundle root (no .md)
    pub title: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DirView {
    /// Directory path relative to the root; "" for the root.
    pub dir: String,
    pub concepts: Vec<DirEntry>,
    pub subdirs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub date: String, // YYYY-MM-DD
    pub at: String,
    pub word: &'static str,
    /// None for erasures, which name no path or title (§8).
    pub path: Option<String>,
    pub title: Option<String>,
    pub actor: String,
}

pub trait Profile: Send + Sync {
    fn id(&self) -> &'static str;
    fn okf_version(&self) -> &'static str;
    fn is_reserved(&self, file_name: &str) -> bool {
        file_name == "index.md" || file_name == "log.md"
    }
    fn managed_keys(&self) -> &[&'static str];
    fn validate(&self, c: &ConceptView) -> Vec<Finding>;
    /// Render the full frontmatter block including both `---` delimiter lines.
    fn render_frontmatter(&self, c: &ConceptView, identity_key: &str) -> String;
    fn render_index(&self, dir: &DirView) -> String;
    fn render_log(&self, dir: &DirView, log: &[LogEntry]) -> String;
}

pub fn by_id(id: &str) -> Option<Box<dyn Profile>> {
    match id {
        "okf-0.2" => Some(Box::new(Okf02)),
        "okf-0.1" => Some(Box::new(Okf01)),
        _ => None,
    }
}

pub fn by_version(v: &str) -> Option<Box<dyn Profile>> {
    match v {
        "0.2" => Some(Box::new(Okf02)),
        "0.1" => Some(Box::new(Okf01)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------

pub struct Okf02;
pub struct Okf01;

const MANAGED_02: &[&str] = &["wisp_id", "generated", "verified", "status", "sources"];
const MANAGED_01: &[&str] = &["wisp_id", "timestamp", "status"];
const LEADING_KEYS: &[&str] = &["type", "title", "description", "resource", "tags"];

fn yaml_block(key: &str, value: &serde_yaml::Value, le: &str) -> String {
    wisp_parse::frontmatter::format_entry(key, value, le)
}

fn json_to_yaml(v: &serde_json::Value) -> serde_yaml::Value {
    serde_yaml::to_value(v).unwrap_or(serde_yaml::Value::Null)
}

fn render_fm_common(
    c: &ConceptView,
    identity_key: &str,
    managed_block: impl Fn(&str) -> String,
) -> String {
    let le = if c.line_ending.is_empty() {
        "\n"
    } else {
        c.line_ending.as_str()
    };
    let mut out = String::new();
    out.push_str("---");
    out.push_str(le);
    if let Some(id) = c.wisp_id {
        out.push_str(&format!("{identity_key}: {id}{le}"));
    }
    // Insert managed keys after the last leading key present, else right after wisp_id.
    let last_leading = c
        .entries
        .iter()
        .rposition(|(k, _)| LEADING_KEYS.contains(&k.as_str()));
    let managed = managed_block(le);
    let mut inserted = false;
    if last_leading.is_none() {
        out.push_str(&managed);
        inserted = true;
    }
    for (i, (_, raw)) in c.entries.iter().enumerate() {
        out.push_str(raw);
        if !raw.ends_with('\n') {
            out.push_str(le);
        }
        if !inserted && Some(i) == last_leading {
            out.push_str(&managed);
            inserted = true;
        }
    }
    if !inserted {
        out.push_str(&managed);
    }
    out.push_str("---");
    out.push_str(le);
    out
}

fn common_validate(c: &ConceptView, require_sources_match: bool) -> Vec<Finding> {
    let mut f = Vec::new();
    match &c.type_ {
        Some(t) if !t.trim().is_empty() => {}
        _ => f.push(Finding {
            level: Level::Must,
            code: "type_required".into(),
            message: "frontmatter `type` is required and must be non-empty".into(),
            path: c.path.clone(),
        }),
    }
    if let Some(s) = &c.status {
        let _ = s;
    }
    for (k, raw) in &c.entries {
        if k == "status" {
            if let Some(v) = wisp_parse::frontmatter::entry_string(raw) {
                if Status::parse(&v).is_none() {
                    f.push(Finding {
                        level: Level::Should,
                        code: "status_value".into(),
                        message: format!(
                            "status should be draft, stable, or deprecated (got {v:?})"
                        ),
                        path: c.path.clone(),
                    });
                }
            }
        }
        if k == "timestamp" {
            if let Some(v) = wisp_parse::frontmatter::entry_string(raw) {
                if parse_rfc3339(&v).is_none() {
                    f.push(Finding {
                        level: Level::Should,
                        code: "timestamp_format".into(),
                        message: "timestamps should be RFC 3339 with an offset".into(),
                        path: c.path.clone(),
                    });
                }
            }
        }
    }
    if let Some((by, _)) = &c.generated {
        if ActorKind::classify(by).is_none() {
            f.push(Finding {
                level: Level::Should,
                code: "actor_format".into(),
                message: format!(
                    "actor {by:?} should follow <producer>/<version>, human:<id>, or process:<id>"
                ),
                path: c.path.clone(),
            });
        }
    }
    if require_sources_match {
        for label in &c.footnotes {
            if !c.source_ids.iter().any(|s| s == label) {
                f.push(Finding {
                    level: Level::Should,
                    code: "footnote_unresolved".into(),
                    message: format!("footnote [^{label}] does not resolve to a sources[].id"),
                    path: c.path.clone(),
                });
            }
        }
    }
    f
}

fn render_index_common(dir: &DirView, version: Option<&str>) -> String {
    let mut out = String::new();
    if dir.dir.is_empty() {
        if let Some(v) = version {
            out.push_str(&format!("---\nokf_version: \"{v}\"\n---\n"));
        }
    }
    let name = if dir.dir.is_empty() {
        "Index".to_string()
    } else {
        dir.dir.rsplit('/').next().unwrap_or(&dir.dir).to_string()
    };
    out.push_str(&format!("# {name}\n\n"));
    if !dir.concepts.is_empty() {
        out.push_str("## Concepts\n\n");
        for c in &dir.concepts {
            let file = c.path.rsplit('/').next().unwrap_or(&c.path);
            let title = c.title.clone().unwrap_or_else(|| file.to_string());
            match &c.description {
                Some(d) if !d.trim().is_empty() => {
                    out.push_str(&format!("- [{title}]({file}.md) — {}\n", d.trim()))
                }
                _ => out.push_str(&format!("- [{title}]({file}.md)\n")),
            }
        }
        out.push('\n');
    }
    if !dir.subdirs.is_empty() {
        out.push_str("## Directories\n\n");
        for s in &dir.subdirs {
            let last = s.rsplit('/').next().unwrap_or(s);
            out.push_str(&format!("- [{last}/]({last}/index.md)\n"));
        }
        out.push('\n');
    }
    out
}

fn render_log_common(dir: &DirView, log: &[LogEntry]) -> String {
    let mut out = String::new();
    let name = if dir.dir.is_empty() {
        "Log".to_string()
    } else {
        format!("Log: {}", dir.dir)
    };
    out.push_str(&format!("# {name}\n"));
    let mut entries: Vec<&LogEntry> = log.iter().collect();
    entries.sort_by(|a, b| b.at.cmp(&a.at));
    let mut current_date = String::new();
    for e in entries {
        if e.date != current_date {
            current_date = e.date.clone();
            out.push_str(&format!("\n## {}\n\n", e.date));
        }
        match (&e.path, &e.title) {
            (Some(p), t) => {
                let rel = p.strip_prefix(&format!("{}/", dir.dir)).unwrap_or(p);
                let title = t.clone().unwrap_or_else(|| rel.to_string());
                out.push_str(&format!(
                    "- {}: [{}]({}.md) by {}\n",
                    e.word, title, rel, e.actor
                ));
            }
            (None, _) => out.push_str(&format!("- {}: an erasure occurred\n", e.word)),
        }
    }
    out
}

impl Profile for Okf02 {
    fn id(&self) -> &'static str {
        "okf-0.2"
    }
    fn okf_version(&self) -> &'static str {
        "0.2"
    }
    fn managed_keys(&self) -> &[&'static str] {
        MANAGED_02
    }
    fn validate(&self, c: &ConceptView) -> Vec<Finding> {
        common_validate(c, true)
    }
    fn render_frontmatter(&self, c: &ConceptView, identity_key: &str) -> String {
        render_fm_common(c, identity_key, |le| {
            let mut m = String::new();
            if let Some((by, at)) = &c.generated {
                let mut map = serde_yaml::Mapping::new();
                map.insert("by".into(), serde_yaml::Value::String(by.clone()));
                map.insert("at".into(), serde_yaml::Value::String(at.clone()));
                m.push_str(&yaml_block(
                    "generated",
                    &serde_yaml::Value::Mapping(map),
                    le,
                ));
            }
            if !c.verified.is_empty() {
                let seq: Vec<serde_yaml::Value> = c
                    .verified
                    .iter()
                    .map(|(by, at)| {
                        let mut map = serde_yaml::Mapping::new();
                        map.insert("by".into(), serde_yaml::Value::String(by.clone()));
                        map.insert("at".into(), serde_yaml::Value::String(at.clone()));
                        serde_yaml::Value::Mapping(map)
                    })
                    .collect();
                m.push_str(&yaml_block(
                    "verified",
                    &serde_yaml::Value::Sequence(seq),
                    le,
                ));
            }
            if let Some(s) = c.status {
                m.push_str(&yaml_block(
                    "status",
                    &serde_yaml::Value::String(s.as_str().into()),
                    le,
                ));
            }
            if !c.sources.is_empty() {
                let seq: Vec<serde_yaml::Value> = c.sources.iter().map(json_to_yaml).collect();
                m.push_str(&yaml_block(
                    "sources",
                    &serde_yaml::Value::Sequence(seq),
                    le,
                ));
            }
            if let Some((at, by, reason)) = &c.retracted {
                let mut map = serde_yaml::Mapping::new();
                map.insert("at".into(), serde_yaml::Value::String(at.clone()));
                map.insert("by".into(), serde_yaml::Value::String(by.clone()));
                map.insert("reason".into(), serde_yaml::Value::String(reason.clone()));
                m.push_str(&yaml_block(
                    "wisp_retracted",
                    &serde_yaml::Value::Mapping(map),
                    le,
                ));
            }
            m
        })
    }
    fn render_index(&self, dir: &DirView) -> String {
        render_index_common(dir, Some("0.2"))
    }
    fn render_log(&self, dir: &DirView, log: &[LogEntry]) -> String {
        render_log_common(dir, log)
    }
}

impl Profile for Okf01 {
    fn id(&self) -> &'static str {
        "okf-0.1"
    }
    fn okf_version(&self) -> &'static str {
        "0.1"
    }
    fn managed_keys(&self) -> &[&'static str] {
        MANAGED_01
    }
    fn validate(&self, c: &ConceptView) -> Vec<Finding> {
        common_validate(c, false)
    }
    fn render_frontmatter(&self, c: &ConceptView, identity_key: &str) -> String {
        render_fm_common(c, identity_key, |le| {
            let mut m = String::new();
            // okf-0.1 carries a single `timestamp` instead of `generated`.
            if let Some((_, at)) = &c.generated {
                m.push_str(&yaml_block(
                    "timestamp",
                    &serde_yaml::Value::String(at.clone()),
                    le,
                ));
            }
            if let Some(s) = c.status {
                m.push_str(&yaml_block(
                    "status",
                    &serde_yaml::Value::String(s.as_str().into()),
                    le,
                ));
            }
            m
        })
    }
    fn render_index(&self, dir: &DirView) -> String {
        render_index_common(dir, Some("0.1"))
    }
    fn render_log(&self, dir: &DirView, log: &[LogEntry]) -> String {
        render_log_common(dir, log)
    }
}

/// Migration plan from okf-0.1 to okf-0.2 (§12). Pure: the engine records the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationStep {
    /// `unset_key timestamp` with origin = (last actor, timestamp value).
    UnsetTimestamp { path: String, timestamp: String },
    /// Parse a `Citations` section's list items into sources, then remove the section.
    CitationsToSources {
        path: String,
        heading: String,
        items: Vec<String>,
    },
}

pub fn plan_migration_01_to_02(concepts: &[ConceptView]) -> Vec<MigrationStep> {
    let mut steps = Vec::new();
    for c in concepts {
        if let Some((_, raw)) = c.entries.iter().find(|(k, _)| k == "timestamp") {
            if let Some(ts) = wisp_parse::frontmatter::entry_string(raw) {
                steps.push(MigrationStep::UnsetTimestamp {
                    path: c.path.clone(),
                    timestamp: ts,
                });
            }
        }
        for (_, heading, body) in &c.sections {
            if heading.trim().eq_ignore_ascii_case("citations") {
                let items: Vec<String> = body
                    .lines()
                    .filter_map(|l| {
                        let t = l.trim_start();
                        t.strip_prefix("- ")
                            .or_else(|| t.strip_prefix("* "))
                            .map(|s| s.trim().to_string())
                    })
                    .filter(|s| !s.is_empty())
                    .collect();
                steps.push(MigrationStep::CitationsToSources {
                    path: c.path.clone(),
                    heading: heading.clone(),
                    items,
                });
            }
        }
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_render_places_managed_keys() {
        let c = ConceptView {
            path: "tables/orders".into(),
            wisp_id: Some(ConceptId::new()),
            type_: Some("Table".into()),
            entries: vec![
                ("type".into(), "type: Table\n".into()),
                ("title".into(), "title: Orders\n".into()),
                ("custom".into(), "custom: 1\n".into()),
            ],
            generated: Some(("human:alex".into(), "2026-10-19T00:00:00Z".into())),
            status: Some(Status::Draft),
            line_ending: "\n".into(),
            ..Default::default()
        };
        let s = Okf02.render_frontmatter(&c, "wisp_id");
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines[0], "---");
        assert!(lines[1].starts_with("wisp_id: "));
        assert_eq!(lines[2], "type: Table");
        assert_eq!(lines[3], "title: Orders");
        assert_eq!(lines[4], "generated:");
        assert!(s.contains("status: draft\n"));
        assert!(s.contains("custom: 1\n"));
        assert!(s.ends_with("---\n"));
        // Re-parse what we rendered.
        let (fm, _) = wisp_parse::frontmatter::split(&format!("{s}body")).unwrap();
        assert_eq!(
            fm.get("generated").map(|r| r.contains("by: human:alex")),
            Some(true)
        );
    }

    #[test]
    fn must_type() {
        let c = ConceptView {
            path: "x".into(),
            type_: None,
            ..Default::default()
        };
        let f = Okf02.validate(&c);
        assert!(f
            .iter()
            .any(|f| f.level == Level::Must && f.code == "type_required"));
    }

    #[test]
    fn migration_plan() {
        let c = ConceptView {
            path: "a".into(),
            entries: vec![(
                "timestamp".into(),
                "timestamp: 2025-01-01T00:00:00Z\n".into(),
            )],
            sections: vec![(2, "Citations".into(), "- Smith 2020\n- Doe 2021\n".into())],
            ..Default::default()
        };
        let p = plan_migration_01_to_02(&[c]);
        assert_eq!(p.len(), 2);
        assert!(
            matches!(&p[1], MigrationStep::CitationsToSources { items, .. } if items.len() == 2)
        );
    }
}
