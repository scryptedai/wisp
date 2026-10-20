//! Divergent ref merge analysis (§11.5).

use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use wisp_model::*;
use wisp_store::Ledger;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum ScopeKey {
    Section(ConceptId, SectionId),
    Frontmatter(ConceptId, String),
    ConceptPath(ConceptId),
    Source(ConceptId, String),
    Asset(AssetId),
    Other(String),
}

pub fn scope_key(c: &Change) -> ScopeKey {
    match &c.target {
        Target::Section { concept, section } => ScopeKey::Section(*concept, *section),
        Target::FrontmatterKey {
            concept,
            key_commit,
        } => ScopeKey::Frontmatter(*concept, hex::encode(key_commit.0)),
        Target::Concept { concept } => ScopeKey::ConceptPath(*concept),
        Target::Source { concept, source } => ScopeKey::Source(*concept, source.clone()),
        Target::Asset { asset } => ScopeKey::Asset(*asset),
        other => ScopeKey::Other(format!("{other:?}")),
    }
}

pub struct DivergenceAnalysis {
    pub conflicts: Vec<Value>,
    pub erase_skips: HashSet<ChangeId>,
    pub erase_reports: Vec<Value>,
}

fn scope_keys_for_changesets(
    conn: &Connection,
    ids: &[ChangesetId],
) -> Result<HashMap<ScopeKey, ChangeId>> {
    let mut map = HashMap::new();
    for cs in ids {
        for c in Ledger::changes_of(conn, *cs)? {
            if c.op == Op::Verify {
                continue;
            }
            let key = scope_key(&c);
            map.insert(key, c.id);
        }
    }
    Ok(map)
}

fn erased_scopes_since(conn: &Connection, ids: &[ChangesetId]) -> Result<HashSet<ScopeId>> {
    let mut out = HashSet::new();
    for cs in ids {
        for c in Ledger::changes_of(conn, *cs)? {
            if c.op == Op::Erase {
                if let Target::Erase { scopes } = &c.target {
                    out.extend(scopes.iter().copied());
                }
                out.insert(c.scope);
            }
        }
    }
    Ok(out)
}

pub fn changes_same_commitment(
    conn: &Connection,
    ours: ChangeId,
    theirs: ChangeId,
) -> Result<bool> {
    let a = Ledger::get_change(conn, ours)?
        .ok_or_else(|| WispError::internal("merge conflict scan: missing change"))?;
    let b = Ledger::get_change(conn, theirs)?
        .ok_or_else(|| WispError::internal("merge conflict scan: missing change"))?;
    if a.target != b.target || a.op != b.op {
        return Ok(false);
    }
    Ok(match (&a.payload, &b.payload) {
        (Some(pa), Some(pb)) => pa.commit == pb.commit,
        (None, None) => true,
        _ => false,
    })
}

pub fn analyze_divergence(
    conn: &Connection,
    into_cs: &[ChangesetId],
    source_cs: &[ChangesetId],
) -> Result<DivergenceAnalysis> {
    let into_keys = scope_keys_for_changesets(conn, into_cs)?;
    let source_keys = scope_keys_for_changesets(conn, source_cs)?;
    let into_erased = erased_scopes_since(conn, into_cs)?;
    let source_erased = erased_scopes_since(conn, source_cs)?;

    let mut erase_skips = HashSet::new();
    let mut erase_reports = Vec::new();
    let mut conflicts = Vec::new();
    let all_keys: HashSet<&ScopeKey> = into_keys.keys().chain(source_keys.keys()).collect();
    for key in all_keys {
        let ours = into_keys.get(key);
        let theirs = source_keys.get(key);
        match (ours, theirs) {
            (None, None) => {}
            (Some(ours), None) => {
                let our_change = Ledger::get_change(conn, *ours)?.unwrap();
                if source_erased.contains(&our_change.scope) {
                    erase_skips.insert(*ours);
                    erase_reports.push(json!({
                        "rule": "erase_wins",
                        "dropped": ours.to_string(),
                        "scope": format!("{key:?}"),
                        "reason": "erased_on_source",
                    }));
                }
            }
            (None, Some(theirs)) => {
                let their_change = Ledger::get_change(conn, *theirs)?.unwrap();
                if into_erased.contains(&their_change.scope) {
                    erase_skips.insert(*theirs);
                    erase_reports.push(json!({
                        "rule": "erase_wins",
                        "dropped": theirs.to_string(),
                        "scope": format!("{key:?}"),
                        "reason": "erased_on_into",
                    }));
                }
            }
            (Some(ours), Some(theirs)) => {
                if ours == theirs || changes_same_commitment(conn, *ours, *theirs)? {
                    continue;
                }
                let our_change = Ledger::get_change(conn, *ours)?.unwrap();
                let their_change = Ledger::get_change(conn, *theirs)?.unwrap();
                if into_erased.contains(&their_change.scope) {
                    erase_skips.insert(*theirs);
                    erase_reports.push(json!({
                        "rule": "erase_wins",
                        "dropped": theirs.to_string(),
                        "scope": format!("{key:?}"),
                        "reason": "erased_on_into",
                    }));
                    continue;
                }
                if source_erased.contains(&our_change.scope) {
                    erase_skips.insert(*ours);
                    erase_reports.push(json!({
                        "rule": "erase_wins",
                        "dropped": ours.to_string(),
                        "scope": format!("{key:?}"),
                        "reason": "erased_on_source",
                    }));
                    continue;
                }
                conflicts.push(json!({
                    "type": "scope_conflict",
                    "scope": format!("{key:?}"),
                    "ours": ours.to_string(),
                    "theirs": theirs.to_string(),
                }));
            }
        }
    }
    conflicts.extend(retract_vs_content_conflicts(conn, into_cs, source_cs)?);

    Ok(DivergenceAnalysis {
        conflicts,
        erase_skips,
        erase_reports,
    })
}

/// Concept retract on one branch vs section body change on the other (§11.5).
fn retract_vs_content_conflicts(
    conn: &Connection,
    into_cs: &[ChangesetId],
    source_cs: &[ChangesetId],
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    let into_retracts = concept_retracts(conn, into_cs)?;
    let source_retracts = concept_retracts(conn, source_cs)?;
    let into_edits = section_edits_by_concept(conn, into_cs)?;
    let source_edits = section_edits_by_concept(conn, source_cs)?;
    for (concept, change) in &source_edits {
        if into_retracts.contains_key(concept) {
            out.push(json!({
                "type": "scope_conflict",
                "scope": format!("retract_vs_content concept={concept}"),
                "ours": into_retracts[concept].to_string(),
                "theirs": change.to_string(),
            }));
        }
    }
    for (concept, change) in &into_edits {
        if source_retracts.contains_key(concept) {
            out.push(json!({
                "type": "scope_conflict",
                "scope": format!("retract_vs_content concept={concept}"),
                "ours": change.to_string(),
                "theirs": source_retracts[concept].to_string(),
            }));
        }
    }
    Ok(out)
}

fn concept_retracts(
    conn: &Connection,
    ids: &[ChangesetId],
) -> Result<HashMap<ConceptId, ChangeId>> {
    let mut m = HashMap::new();
    for cs in ids {
        for c in Ledger::changes_of(conn, *cs)? {
            if c.op == Op::Retract {
                if let Target::Concept { concept } = c.target {
                    m.insert(concept, c.id);
                }
            }
        }
    }
    Ok(m)
}

fn section_edits_by_concept(
    conn: &Connection,
    ids: &[ChangesetId],
) -> Result<HashMap<ConceptId, ChangeId>> {
    let mut m = HashMap::new();
    for cs in ids {
        for c in Ledger::changes_of(conn, *cs)? {
            if matches!(c.op, Op::UpdateSection | Op::AddSection | Op::Split) {
                if let Target::Section { concept, .. } = c.target {
                    m.insert(concept, c.id);
                }
            }
        }
    }
    Ok(m)
}
