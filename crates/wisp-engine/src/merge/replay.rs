//! Merge-aware changeset ordering for hot rebuild (§11.5).

use rusqlite::Connection;
use std::collections::HashSet;
use wisp_model::ChangesetId;
use wisp_store::Ledger;

use super::{changesets_since, dedupe_changesets_preserve_order, merge_base};

/// Changeset ids to replay for `head`, oldest first.
///
/// Linear history uses ledger closure order. Two-parent merge heads expand to
/// `base` closure + each parent's delta since `base`, recursively for nested merges.
pub fn ordered_changeset_ids(
    conn: &Connection,
    head: ChangesetId,
) -> wisp_model::Result<Vec<ChangesetId>> {
    layered_changesets(conn, head)
}

fn layered_changesets(
    conn: &Connection,
    head: ChangesetId,
) -> wisp_model::Result<Vec<ChangesetId>> {
    let row = Ledger::get_changeset(conn, head)?
        .ok_or_else(|| wisp_model::WispError::internal("missing head changeset"))?;
    if row.changeset.parents.len() == 2 {
        let into_head = row.changeset.parents[0];
        let source_head = row.changeset.parents[1];
        let base = merge_base(conn, into_head, source_head)?
            .ok_or_else(|| wisp_model::WispError::integrity("merge without base"))?;
        let mut out = layered_changesets(conn, base)?;
        out.extend(changesets_since(conn, into_head, base));
        out.extend(changesets_since(conn, source_head, base));
        out.push(head);
        return Ok(dedupe_changesets_preserve_order(out));
    }

    let closure = Ledger::changesets_closure(conn, head)?;
    let mut out = Vec::new();
    let mut expanded = HashSet::new();
    for row in closure {
        let id = row.changeset.id;
        if row.changeset.parents.len() == 2 && expanded.insert(id) {
            let layered = layered_changesets(conn, id)?;
            for cs in layered {
                if !out.contains(&cs) {
                    out.push(cs);
                }
            }
        } else if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}
