use crate::model::SiteModel;

/// Second pass: enforce `--no-content` and drop empty optional fields before render.
pub fn redact_site_model(model: &mut SiteModel) {
    if model.content {
        return;
    }
    for c in &mut model.concepts {
        c.markdown_html = None;
    }
    for s in &mut model.sections {
        for v in &mut s.versions {
            v.body = None;
            v.diff_vs_predecessor = None;
            v.diff_missing_baseline = false;
        }
    }
}
