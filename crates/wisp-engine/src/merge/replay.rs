//! Merge-aware changeset ordering for hot rebuild (§11.5).

use rusqlite::Connection;
use wisp_model::ChangesetId;
use wisp_store::Ledger;

use super::{changesets_since, dedupe_changesets_preserve_order, merge_base};

pub fn ordered_changeset_ids(
    conn: &Connection,
    head: ChangesetId,
) -> wisp_model::Result<Vec<ChangesetId>> {
    let row = Ledger::get_changeset(conn, head)?
        .ok_or_else(|| wisp_model::WispError::internal("missing head changeset"))?;
    if row.changeset.parents.len() != 2 {
        return Ok(Ledger::changesets_closure(conn, head)?
            .into_iter()
            .map(|r| r.changeset.id)
            .collect());
    }
    let into_head = row.changeset.parents[0];
    let source_head = row.changeset.parents[1];
    let base = merge_base(conn, into_head, source_head)?
        .ok_or_else(|| wisp_model::WispError::integrity("merge without base"))?;
    let mut out: Vec<ChangesetId> = Ledger::changesets_closure(conn, base)?
        .into_iter()
        .map(|r| r.changeset.id)
        .collect();
    out.extend(changesets_since(conn, into_head, base));
    out.extend(changesets_since(conn, source_head, base));
    out.push(head);
    Ok(dedupe_changesets_preserve_order(out))
}
