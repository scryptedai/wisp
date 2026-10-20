//! OKF fixtures and catalog for test-driven completion of wisp (§17 test corpus).
//!
//! All paths are relative to `data/` next to this crate's manifest.

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use walkdir::WalkDir;

#[derive(Debug, Error)]
pub enum CorpusError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("walk: {0}")]
    Walk(#[from] walkdir::Error),
    #[error("catalog: {0}")]
    Catalog(String),
    #[error("unknown bundle `{0}`")]
    UnknownBundle(String),
    #[error("unknown scenario `{0}`")]
    UnknownScenario(String),
}

/// Root of embedded corpus files (`crates/wisp-corpus/data`).
pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
}

pub fn catalog_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("catalog.toml")
}

#[derive(Debug, Clone, Deserialize)]
pub struct BundleSpec {
    pub id: String,
    pub path: String,
    pub profile: String,
    pub description: String,
    pub invariants: Vec<String>,
    pub record: bool,
    pub lossless_strip: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConceptSpec {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub expect_frontmatter: Option<bool>,
    pub expect_opaque_frontmatter: Option<bool>,
    /// Minimum `parse_file().sections().len()` (preamble counts as a section).
    pub expect_min_sections: Option<usize>,
    pub expect_max_sections: Option<usize>,
    /// Expected `sectioned.erasure_markers.len()`.
    pub expect_erasure_markers: Option<usize>,
    /// Substrings that must appear in `sectioned.warnings`.
    pub expect_warning_contains: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScenarioSpec {
    pub id: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub bundle: Vec<BundleSpec>,
    pub concept: Vec<ConceptSpec>,
    pub scenario: Vec<ScenarioSpec>,
}

impl Catalog {
    pub fn load() -> Result<Self, CorpusError> {
        let raw = fs::read_to_string(catalog_path())?;
        toml::from_str(&raw).map_err(|e| CorpusError::Catalog(e.to_string()))
    }

    pub fn bundle(&self, id: &str) -> Option<&BundleSpec> {
        self.bundle.iter().find(|b| b.id == id)
    }

    pub fn scenario(&self, id: &str) -> Option<&ScenarioSpec> {
        self.scenario.iter().find(|s| s.id == id)
    }
}

pub fn bundle_root(spec: &BundleSpec) -> PathBuf {
    data_dir().join(&spec.path)
}

/// Copy a catalog bundle tree into `dest` (the future bundle root, not `.wisp/`).
pub fn copy_bundle(spec: &BundleSpec, dest: &Path) -> Result<(), CorpusError> {
    let src = bundle_root(spec);
    if !src.is_dir() {
        return Err(CorpusError::Catalog(format!(
            "missing bundle dir {}",
            src.display()
        )));
    }
    copy_tree(&src, dest)?;
    Ok(())
}

fn copy_tree(src: &Path, dest: &Path) -> Result<(), CorpusError> {
    for e in WalkDir::new(src) {
        let e = e?;
        let rel = e.path().strip_prefix(src).unwrap();
        let target = dest.join(rel);
        if e.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(p) = target.parent() {
                fs::create_dir_all(p)?;
            }
            fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

/// All concept `.md` files under a bundle (skips generated names at any depth).
pub fn concept_markdown_files(bundle_root: &Path) -> Result<Vec<PathBuf>, CorpusError> {
    let mut out = Vec::new();
    for e in WalkDir::new(bundle_root) {
        let e = e?;
        if !e.file_type().is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy();
        if name == "index.md" || name == "log.md" {
            continue;
        }
        if name.ends_with(".md") {
            out.push(e.path().to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

pub fn read_concept(spec: &ConceptSpec) -> Result<String, CorpusError> {
    let p = data_dir().join(&spec.path);
    Ok(fs::read_to_string(p)?)
}

/// Snapshot concept bodies before `wisp record` (relative path → text).
pub fn snapshot_bundle_sources(
    bundle_root: &Path,
) -> Result<std::collections::HashMap<String, String>, CorpusError> {
    let mut map = std::collections::HashMap::new();
    for p in concept_markdown_files(bundle_root)? {
        let rel = p
            .strip_prefix(bundle_root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        map.insert(rel, fs::read_to_string(p)?);
    }
    Ok(map)
}

/// Lines wisp adds on record that must be stripped before byte-compare (I3 checkout --strip-anchors).
pub fn strip_managed_and_anchors(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with("wisp_id: ") {
            continue;
        }
        if line.starts_with("generated:") || line.starts_with("verified:") {
            continue;
        }
        if line.starts_with("  by: ") || line.starts_with("  at: ") {
            continue;
        }
        if line.starts_with("  - by: ") || line.starts_with("    at: ") {
            continue;
        }
        if line.contains("<!-- ^") && line.trim_end().ends_with("-->") {
            let trimmed = line.split("<!-- ^").next().unwrap_or(line).trim_end();
            out.push_str(trimmed);
            out.push('\n');
            continue;
        }
        if line.trim().starts_with("<!-- ^") && line.trim().ends_with("-->") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ScenarioCtx {
    #[serde(default)]
    pub erased_sections: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MatchScenario {
    pub id: String,
    pub description: Option<String>,
    pub input: String,
    #[serde(default)]
    pub prior: Vec<PriorSectionSpec>,
    #[serde(default)]
    pub expect: Vec<ExpectDecision>,
    #[serde(default)]
    pub ctx: ScenarioCtx,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PriorSectionSpec {
    pub id: String,
    pub level: u8,
    pub heading: String,
    pub heading_line: String,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExpectDecision {
    pub index: usize,
    pub rule_contains: String,
}

pub fn load_match_scenario(spec: &ScenarioSpec) -> Result<MatchScenario, CorpusError> {
    let dir = data_dir().join(&spec.path);
    let toml_path = dir.join("scenario.toml");
    let raw = fs::read_to_string(toml_path)?;
    let mut scenario: MatchScenario =
        toml::from_str(&raw).map_err(|e| CorpusError::Catalog(e.to_string()))?;
    if !scenario.input.contains('\n') {
        scenario.input = fs::read_to_string(dir.join(&scenario.input))?;
    }
    Ok(scenario)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses() {
        let c = Catalog::load().unwrap();
        assert!(c.bundle.len() >= 11);
        assert!(c.concept.len() >= 8);
        assert!(c.scenario.len() >= 8);
    }
}
