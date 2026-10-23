use serde::{Deserialize, Serialize};

pub const PLAINTEXT_NOTICE: &str =
    "This export is a plaintext copy. Later wisp erasures cannot remove content from this folder or copies of it.";

pub const SCHEMA_VERSION: &str = "wisp-html-export/1";
pub const RENDERER_VERSION: &str = "0.1.0";
pub const MAX_SECTION_VERSIONS: usize = 20;
pub const MAX_INPUTS_PER_VERSION: usize = 12;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SiteModel {
    pub ref_name: String,
    pub head: Option<String>,
    pub content: bool,
    pub overview: Overview,
    pub concepts: Vec<ConceptPage>,
    pub sections: Vec<SectionPage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Overview {
    pub concept_count: usize,
    pub section_count: usize,
    pub concepts: Vec<ConceptLink>,
    pub retractions: Vec<RetractionRow>,
    pub erasures: Vec<ErasureMarkerRow>,
    pub demo_concept_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConceptLink {
    pub id: String,
    pub path: String,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetractionRow {
    pub concept_id: String,
    pub path: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErasureMarkerRow {
    pub concept_id: String,
    pub concept_path: String,
    pub section_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConceptPage {
    pub id: String,
    pub path: String,
    pub type_name: String,
    pub status: Option<String>,
    pub retracted: bool,
    pub markdown_html: Option<String>,
    pub sections: Vec<SectionLink>,
    pub badges: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionLink {
    pub id: String,
    pub heading: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionPage {
    pub id: String,
    pub concept_id: String,
    pub concept_path: String,
    pub heading: String,
    pub versions: Vec<SectionVersion>,
    pub total_versions: usize,
    pub versions_truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionVersion {
    pub change_id: String,
    pub changeset_id: String,
    pub actor: String,
    pub at: String,
    pub inputs_complete: bool,
    pub inputs: Vec<InputRow>,
    pub inputs_truncated: bool,
    pub not_kept_at_merge: Vec<String>,
    pub body: Option<String>,
    pub diff_vs_predecessor: Option<String>,
    pub diff_missing_baseline: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputRow {
    pub label: String,
    pub state: String,
}
