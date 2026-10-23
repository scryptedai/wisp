//! Render a redacted [`SiteModel`] to static HTML (no `Repository`, keys, or DB).

mod diff;
mod escape;
mod model;
mod redact;
mod render;
mod sanitize;

pub use diff::line_diff;
pub use model::*;
pub use redact::redact_site_model;
pub use render::{markdown_to_html, render_receipt, render_site, validate_page_id, RenderSummary};
pub use sanitize::sanitize_markdown_html;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn line_diff_shows_changes() {
        let d = line_diff("a\nb\n", "a\nc\n");
        assert!(d.contains("- b"));
        assert!(d.contains("+ c"));
    }

    #[test]
    fn render_minimal_site() {
        let model = SiteModel {
            ref_name: "main".into(),
            head: Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".into()),
            content: true,
            overview: Overview {
                concept_count: 0,
                section_count: 0,
                concepts: vec![],
                retractions: vec![],
                erasures: vec![],
                demo_concept_id: None,
            },
            concepts: vec![],
            sections: vec![],
        };
        let dir = tempfile::tempdir().unwrap();
        let s = render_site(&model, dir.path()).unwrap();
        assert_eq!(s.page_count, 1);
        assert!(dir.path().join("index.html").is_file());
        assert!(dir.path().join("manifest.json").is_file());
    }

    #[test]
    fn validate_page_id_rejects_paths() {
        assert!(validate_page_id("notes/foo.md").is_err());
        assert!(validate_page_id("01ARZ3NDEKTSV4RRFFQ69G5FAV").is_ok());
    }

    #[test]
    fn redact_strips_content_fields() {
        let mut model = SiteModel {
            ref_name: "main".into(),
            head: None,
            content: false,
            overview: Overview {
                concept_count: 0,
                section_count: 0,
                concepts: vec![],
                retractions: vec![],
                erasures: vec![],
                demo_concept_id: None,
            },
            concepts: vec![],
            sections: vec![SectionPage {
                id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
                concept_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
                concept_path: "x".into(),
                heading: "h".into(),
                versions: vec![SectionVersion {
                    change_id: "c".into(),
                    changeset_id: "cs".into(),
                    actor: "a".into(),
                    at: "t".into(),
                    inputs_complete: false,
                    inputs: vec![],
                    inputs_truncated: false,
                    not_kept_at_merge: vec![],
                    body: Some("secret".into()),
                    diff_vs_predecessor: Some("+x".into()),
                    diff_missing_baseline: true,
                }],
                total_versions: 1,
                versions_truncated: false,
            }],
        };
        redact_site_model(&mut model);
        assert!(model.sections[0].versions[0].body.is_none());
        assert!(model.sections[0].versions[0].diff_vs_predecessor.is_none());
    }
}
