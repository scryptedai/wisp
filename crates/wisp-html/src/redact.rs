use crate::model::SiteModel;

/// Second pass: enforce `--no-content` / `--ids-only` before render.
pub fn redact_site_model(model: &mut SiteModel) {
    if model.ids_only {
        apply_ids_only(model);
    }
    if model.content {
        return;
    }
    strip_content(model);
}

pub fn apply_ids_only(model: &mut SiteModel) {
    for c in &mut model.concepts {
        c.path = c.id.clone();
        c.markdown_html = None;
        for s in &mut c.sections {
            s.heading = s.id.clone();
        }
    }
    for s in &mut model.sections {
        s.concept_path = s.concept_id.clone();
        s.heading = s.id.clone();
        for v in &mut s.versions {
            v.actor = mask_actor(&v.actor);
            for inp in &mut v.inputs {
                inp.label = inp.state.clone();
            }
        }
    }
    for s in &mut model.section_pages {
        s.concept_path = s.concept_id.clone();
        s.heading = s.id.clone();
        for v in &mut s.versions {
            v.actor = mask_actor(&v.actor);
            for inp in &mut v.inputs {
                inp.label = inp.state.clone();
            }
        }
    }
    for r in &mut model.overview.retractions {
        r.path = r.concept_id.clone();
        r.reason = None;
    }
    for e in &mut model.overview.erasures {
        e.concept_path = e.concept_id.clone();
        e.reason = e.section_id.clone();
    }
    for c in &mut model.overview.concepts {
        c.path = c.id.clone();
    }
    for row in &mut model.activity {
        row.actor = mask_actor(&row.actor);
        row.target = row.change_id.clone();
    }
    for row in &mut model.actors {
        row.display = row.actor_id.clone();
    }
    for row in &mut model.removals {
        row.reason = None;
    }
}

fn mask_actor(actor: &str) -> String {
    if actor.len() <= 12 {
        return actor.to_string();
    }
    format!("{}…", actor.chars().take(12).collect::<String>())
}

fn strip_content(model: &mut SiteModel) {
    for c in &mut model.concepts {
        c.markdown_html = None;
    }
    for s in &mut model.sections {
        strip_section_versions(s);
    }
    for s in &mut model.section_pages {
        strip_section_versions(s);
    }
    model.graphs.clear();
}

fn strip_section_versions(s: &mut crate::model::SectionPage) {
    for v in &mut s.versions {
        v.body = None;
        v.diff_vs_predecessor = None;
        v.word_diff_vs_predecessor = None;
        v.diff_missing_baseline = false;
    }
}
