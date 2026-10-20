//! Applying changes to a hot tier, link resolution, and hot-tier rebuild (§6.3).

use crate::payloads::*;
use crate::repo::{Reader, Repository};
use rusqlite::Connection;
use wisp_model::*;
use wisp_store::{ConceptState, Hot, Ledger, SectionState};

/// Resolve a link destination written in `from_path`'s file to a concept path.
pub fn resolve_link_path(
    from_concept_path: &str,
    dest: &str,
    form: &wisp_parse::LinkForm,
) -> Option<String> {
    use wisp_parse::LinkForm;
    let dest = dest
        .split('#')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("");
    if dest.is_empty() {
        return None;
    }
    let joined = match form {
        LinkForm::External => return None,
        LinkForm::Absolute => dest.trim_start_matches('/').to_string(),
        LinkForm::Relative => {
            let dir = match from_concept_path.rfind('/') {
                Some(i) => &from_concept_path[..i],
                None => "",
            };
            if dir.is_empty() {
                dest.to_string()
            } else {
                format!("{dir}/{dest}")
            }
        }
    };
    // Normalize `.` and `..` segments.
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut p = parts.join("/");
    if let Some(s) = p.strip_suffix(".md") {
        p = s.to_string();
    }
    Some(p)
}

/// Re-index a section's links into `state_links`.
pub fn index_links(
    conn: &Connection,
    concept: ConceptId,
    concept_path: &str,
    section: SectionId,
    body: &str,
) -> Result<()> {
    let parsed = wisp_parse::sections::section(body);
    let mut rows = Vec::new();
    for s in &parsed.sections {
        for l in &s.links {
            if l.form == wisp_parse::LinkForm::External {
                continue;
            }
            let Some(path) = resolve_link_path(concept_path, &l.dest, &l.form) else {
                continue;
            };
            let target = Hot::concept_by_path(conn, &path)?
                .or(Hot::concept_by_alias(conn, &path)?)
                .map(|c| c.concept);
            let form = match l.form {
                wisp_parse::LinkForm::Absolute => "absolute",
                wisp_parse::LinkForm::Relative => "relative",
                wisp_parse::LinkForm::External => "external",
            };
            rows.push((
                target,
                if target.is_none() { Some(path) } else { None },
                form.to_string(),
            ));
        }
    }
    Hot::replace_links(conn, concept, section, &rows)
}

/// Resolve wanted links that now point at an existing concept (after create/move).
fn resolve_wanted(conn: &Connection, path: &str, concept: ConceptId) -> Result<()> {
    conn.execute(
        "UPDATE state_links SET target_concept = ?2, target_path = NULL WHERE target_concept IS NULL AND target_path = ?1",
        rusqlite::params![path, concept.bytes().to_vec()],
    )
    .map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

fn touch_concept(
    conn: &Connection,
    concept: ConceptId,
    cs: &Changeset,
    c: &Change,
    content: bool,
) -> Result<()> {
    if let Some(mut st) = Hot::concept(conn, concept)? {
        st.version = c.id;
        if content {
            match &c.origin {
                Some(o) => {
                    st.gen_actor = Some(o.actor_id);
                    st.gen_at = Some(o.at.clone());
                }
                None => {
                    st.gen_actor = Some(cs.actor_id);
                    st.gen_at = Some(cs.at.clone());
                }
            }
            st.content_version = Some(c.id);
        }
        Hot::upsert_concept(conn, &st)?;
    }
    Ok(())
}

/// Apply one change to a hot tier. `plaintext` is the decrypted payload, when the change has one
/// and its scope is not erased.
pub fn apply_change(
    conn: &Connection,
    cs: &Changeset,
    c: &Change,
    plaintext: Option<&[u8]>,
) -> Result<()> {
    // A change to a concept the tier does not hold (erased, or created in an erased scope) applies to nothing.
    if c.op != Op::CreateConcept && c.op != Op::Restore {
        if let Some(cp) = c.target.concept() {
            if Hot::concept(conn, cp)?.is_none() {
                return Ok(());
            }
        }
    }
    // An erased payload still happened: the concept's version and `generated` reflect it, the content does not.
    if plaintext.is_none() && c.payload.is_some() {
        if let Some(cp) = c.target.concept() {
            if c.op == Op::RemoveSection {
                if let Some(s) = c.target.section() {
                    Hot::delete_section(conn, s)?;
                }
            }
            touch_concept(conn, cp, cs, c, c.op.is_content_change())?;
        }
        return Ok(());
    }
    match (&c.op, &c.target) {
        (Op::CreateConcept, Target::Concept { concept }) => {
            let path = String::from_utf8_lossy(plaintext.unwrap_or_default()).into_owned();
            Hot::upsert_concept(
                conn,
                &ConceptState {
                    concept: *concept,
                    path: path.clone(),
                    type_: None,
                    status: None,
                    scope: c.scope,
                    version: c.id,
                    gen_actor: Some(c.origin.as_ref().map(|o| o.actor_id).unwrap_or(cs.actor_id)),
                    gen_at: Some(
                        c.origin
                            .as_ref()
                            .map(|o| o.at.clone())
                            .unwrap_or_else(|| cs.at.clone()),
                    ),
                    content_version: Some(c.id),
                },
            )?;
            resolve_wanted(conn, &path, *concept)?;
        }
        (Op::AddSection | Op::Split | Op::MoveSection, Target::Section { concept, section }) => {
            let Some(pt) = plaintext else { return Ok(()) };
            let sb = SectionBody::decode(pt)?;
            if c.op == Op::MoveSection {
                // Leaves its previous owner.
                Hot::delete_section(conn, *section)?;
            }
            let st = SectionState {
                concept: *concept,
                section: *section,
                idx: 0,
                level: sb.level,
                heading: sb.heading,
                heading_line: sb.heading_line,
                body: sb.body,
                scope: c.scope,
                version: c.id,
            };
            if sb.level == 0 {
                // The preamble is always first.
                let mut order: Vec<SectionId> = Hot::sections_of(conn, *concept)?
                    .into_iter()
                    .map(|s| s.section)
                    .collect();
                Hot::upsert_section(conn, &st)?;
                order.insert(0, *section);
                Hot::set_section_order(conn, *concept, &order)?;
            } else {
                Hot::insert_section_after(conn, &st, sb.after)?;
            }
            if let Some(cstate) = Hot::concept(conn, *concept)? {
                index_links(conn, *concept, &cstate.path, *section, &st.body)?;
            }
            touch_concept(conn, *concept, cs, c, true)?;
        }
        (Op::UpdateSection, Target::Section { concept, section }) => {
            let Some(pt) = plaintext else { return Ok(()) };
            let sb = SectionBody::decode(pt)?;
            if let Some(mut st) = Hot::section(conn, *section)? {
                st.level = sb.level;
                st.heading = sb.heading;
                st.heading_line = sb.heading_line;
                st.body = sb.body;
                st.version = c.id;
                Hot::upsert_section(conn, &st)?;
                if let Some(cstate) = Hot::concept(conn, *concept)? {
                    index_links(conn, *concept, &cstate.path, *section, &st.body)?;
                }
            }
            touch_concept(conn, *concept, cs, c, true)?;
        }
        (Op::RemoveSection, Target::Section { concept, section }) => {
            Hot::delete_section(conn, *section)?;
            touch_concept(conn, *concept, cs, c, true)?;
        }
        (Op::ReorderSections, Target::Concept { concept }) => {
            if let Some(pt) = plaintext {
                let order = decode_order(pt)?;
                Hot::set_section_order(conn, *concept, &order)?;
            }
            touch_concept(conn, *concept, cs, c, true)?;
        }
        (Op::SetKey, Target::FrontmatterKey { concept, .. }) => {
            let Some(pt) = plaintext else { return Ok(()) };
            let fm = decode_frontmatter(pt)?;
            Hot::set_frontmatter(conn, *concept, &fm.key, &fm.raw, c.id)?;
            if fm.key == "type" {
                if let Some(mut st) = Hot::concept(conn, *concept)? {
                    st.type_ = wisp_parse::frontmatter::entry_string(&fm.raw);
                    Hot::upsert_concept(conn, &st)?;
                }
            }
            touch_concept(conn, *concept, cs, c, true)?;
        }
        (Op::UnsetKey, Target::FrontmatterKey { concept, .. }) => {
            let Some(pt) = plaintext else { return Ok(()) };
            let fm = decode_frontmatter(pt)?;
            Hot::unset_frontmatter(conn, *concept, &fm.key)?;
            if fm.key == "type" {
                if let Some(mut st) = Hot::concept(conn, *concept)? {
                    st.type_ = None;
                    Hot::upsert_concept(conn, &st)?;
                }
            }
            touch_concept(conn, *concept, cs, c, true)?;
        }
        (Op::SetStatus, Target::Concept { concept }) => {
            let s = String::from_utf8_lossy(plaintext.unwrap_or_default()).into_owned();
            if let Some(mut st) = Hot::concept(conn, *concept)? {
                st.status = Status::parse(s.trim());
                st.version = c.id;
                Hot::upsert_concept(conn, &st)?;
            }
        }
        (Op::Verify, Target::Concept { concept }) => {
            if let Some(st) = Hot::concept(conn, *concept)? {
                let cv = st.content_version.unwrap_or(st.version);
                Hot::add_verified(conn, *concept, cs.actor_id, &cs.at, cv)?;
            }
            touch_concept(conn, *concept, cs, c, false)?;
        }
        (Op::SourcePut, Target::Source { concept, .. }) => {
            let Some(pt) = plaintext else { return Ok(()) };
            let sp = decode_source(pt)?;
            Hot::put_source(conn, *concept, &sp.id, &sp.record, c.id)?;
            touch_concept(conn, *concept, cs, c, true)?;
        }
        (Op::SourceRemove, Target::Source { concept, source }) => {
            Hot::remove_source(conn, *concept, source)?;
            touch_concept(conn, *concept, cs, c, true)?;
        }
        (Op::Move, Target::Concept { concept }) => {
            let new_path = String::from_utf8_lossy(plaintext.unwrap_or_default()).into_owned();
            if let Some(mut st) = Hot::concept(conn, *concept)? {
                let old = st.path.clone();
                Hot::add_alias(conn, *concept, &old, cs.id)?;
                st.path = new_path.clone();
                st.version = c.id;
                Hot::upsert_concept(conn, &st)?;
                resolve_wanted(conn, &new_path, *concept)?;
            }
        }
        (Op::Move, Target::Asset { asset }) => {
            let new_path = String::from_utf8_lossy(plaintext.unwrap_or_default()).into_owned();
            if let Some(mut a) = Hot::asset(conn, *asset)? {
                a.path = new_path;
                a.version = c.id;
                Hot::upsert_asset(conn, &a)?;
            }
        }
        (Op::CreateAsset | Op::UpdateAsset, Target::Asset { asset }) => {
            let Some(pt) = plaintext else { return Ok(()) };
            let (path, bytes) = decode_asset(pt)?;
            Hot::upsert_asset(
                conn,
                &wisp_store::AssetState {
                    asset: *asset,
                    path,
                    bytes,
                    scope: c.scope,
                    version: c.id,
                },
            )?;
        }
        (Op::Retract, Target::Concept { concept }) => {
            if let Some(st) = Hot::concept(conn, *concept)? {
                let reason = c.reason.map(|r| r.as_str()).unwrap_or("other");
                Hot::set_retracted(
                    conn,
                    *concept,
                    &st.path,
                    &cs.at,
                    cs.actor_id,
                    reason,
                    "concept",
                )?;
                Hot::delete_concept(conn, *concept)?;
                Hot::unresolve_links_to_path(conn, *concept, &st.path)?;
            }
        }
        (Op::Retract, Target::Asset { asset }) => {
            if let Some(a) = Hot::asset(conn, *asset)? {
                let reason = c.reason.map(|r| r.as_str()).unwrap_or("other");
                Hot::set_retracted(
                    conn,
                    ConceptId::from_bytes(asset.bytes()),
                    &a.path,
                    &cs.at,
                    cs.actor_id,
                    reason,
                    "asset",
                )?;
                Hot::delete_asset(conn, *asset)?;
            }
        }
        (Op::Restore, _) => {
            // Handled by the caller via `replay_entity`, which needs the ledger and keys.
        }
        (Op::Erase, _)
        | (Op::GroupAdd, _)
        | (Op::GroupRemove, _)
        | (Op::RegisterActor, _)
        | (Op::ActorKey, _) => {}
        _ => {}
    }
    Ok(())
}

/// Changesets that contribute to a ref head (merge-aware), oldest first.
pub fn history_chain(
    ledger: &Ledger,
    head: Option<ChangesetId>,
) -> Result<Vec<wisp_store::ChangesetRow>> {
    match head {
        Some(h) => Ledger::changesets_closure(ledger.conn(), h),
        None => Ok(Vec::new()),
    }
}

/// Apply a changeset's changes to a hot tier, reading and decrypting payloads as needed.
/// Changes whose scope is erased are skipped (tombstones dominate).
pub fn apply_changeset(
    reader: &Reader<'_>,
    conn: &Connection,
    cs: &Changeset,
    changes: &[Change],
) -> Result<()> {
    for c in changes {
        if c.op == Op::Restore {
            match &c.target {
                Target::Concept { concept } => replay_concept(reader, conn, *concept)?,
                Target::Asset { asset } => replay_asset(reader, conn, *asset)?,
                _ => {}
            }
            continue;
        }
        let erased = reader.scope_erased(c.scope)?;
        let pt = match (&c.payload, erased) {
            (Some(p), false) => reader.read_payload(c.scope, p)?,
            _ => None,
        };
        if erased && c.payload.is_none() && c.op != Op::RemoveSection {
            // e.g. retract/verify on an erased concept: nothing to apply.
            continue;
        }
        apply_change(conn, cs, c, pt.as_deref())?;
    }
    Ok(())
}

/// Rebuild a concept's hot state from its ledger history (used by restore).
pub fn replay_concept(reader: &Reader<'_>, conn: &Connection, concept: ConceptId) -> Result<()> {
    Hot::clear_retracted(conn, concept)?;
    Hot::delete_concept(conn, concept)?;
    let rows = Ledger::changes_for_concept(reader.conn, concept, None)?;
    let mut by_cs: Vec<(ChangesetId, Vec<Change>)> = Vec::new();
    for (_, c) in rows {
        if matches!(c.op, Op::Retract | Op::Restore) {
            continue;
        }
        match by_cs.last_mut() {
            Some((id, v)) if *id == c.changeset => v.push(c),
            _ => by_cs.push((c.changeset, vec![c])),
        }
    }
    for (cs_id, changes) in by_cs {
        let Some(row) = Ledger::get_changeset(reader.conn, cs_id)? else {
            continue;
        };
        apply_changeset(reader, conn, &row.changeset, &changes)?;
    }
    Ok(())
}

pub fn replay_asset(reader: &Reader<'_>, conn: &Connection, asset: AssetId) -> Result<()> {
    Hot::clear_retracted(conn, ConceptId::from_bytes(asset.bytes()))?;
    let all = Ledger::changes_all(reader.conn)?;
    for (_, c) in all {
        if c.target != (Target::Asset { asset }) || matches!(c.op, Op::Retract | Op::Restore) {
            continue;
        }
        let Some(row) = Ledger::get_changeset(reader.conn, c.changeset)? else {
            continue;
        };
        apply_changeset(reader, conn, &row.changeset, std::slice::from_ref(&c))?;
    }
    Ok(())
}

/// Rebuild the hot tier of `ref_name` from the cold tier (§6.3). Byte-identical given the same log (I2, I13).
pub fn rebuild_hot(repo: &mut Repository, ref_name: &str) -> Result<()> {
    let head = repo.head(ref_name)?;
    let skip = head
        .and_then(|h| crate::merge::merge_skips_for_head(repo, ref_name, h));
    rebuild_hot_inner(repo, ref_name, skip.as_ref())
}

/// Like `rebuild_hot`, but omit specific changes (merge conflict resolution).
pub fn rebuild_hot_skipping(
    repo: &mut Repository,
    ref_name: &str,
    skip: &std::collections::HashSet<ChangeId>,
) -> Result<()> {
    rebuild_hot_inner(repo, ref_name, Some(skip))
}

fn rebuild_hot_inner(
    repo: &mut Repository,
    ref_name: &str,
    skip: Option<&std::collections::HashSet<ChangeId>>,
) -> Result<()> {
    let skip_owned = match skip {
        Some(s) => Some(s.clone()),
        None => {
            let head = repo.head(ref_name)?;
            head.and_then(|h| crate::merge::merge_skips_for_head(repo, ref_name, h))
        }
    };
    let hot_path = repo.hot_path(ref_name);
    let profile_id = repo.profile.id().to_string();
    let payloads_dir = repo.ledger.payloads_dir();
    let repo_id = repo.repo_id();
    let mut hot = repo
        .hots
        .remove(ref_name)
        .map(Ok)
        .unwrap_or_else(|| Hot::open(&hot_path))?;
    let keystore = &repo.keystore;
    // Hold the ledger write lock for the whole rebuild so no commit can land on a tier that is
    // about to be replaced with an older state.
    let result = (|| -> Result<()> {
        let ltx = repo.ledger.begin_immediate()?;
        let head = Ledger::head(&ltx, ref_name)?;
        let max_seq = Ledger::max_seq(&ltx)?;
        let reader = Reader {
            conn: &ltx,
            payloads_dir: payloads_dir.clone(),
            keystore,
            repo_id,
        };
        let chain = match head {
            Some(h) => Ledger::changesets_closure(&ltx, h)?,
            None => Vec::new(),
        };
        let skip = skip_owned.as_ref();
        let tx = hot.begin()?;
        Hot::clear(&tx)?;
        for row in &chain {
            let changes = Ledger::changes_of(&ltx, row.changeset.id)?;
            let changes: Vec<Change> = match skip {
                Some(skip) => changes
                    .into_iter()
                    .filter(|c| !skip.contains(&c.id))
                    .collect(),
                None => changes,
            };
            apply_changeset(&reader, &tx, &row.changeset, &changes)?;
        }
        // Erasure markers (positions kept as bookkeeping).
        for (section, concept, after, reason) in Ledger::erasure_markers(&ltx)? {
            if Hot::concept(&tx, concept)?.is_some() {
                Hot::add_erased_marker(&tx, concept, section, after, reason.as_str())?;
            }
        }
        Hot::set_meta(
            &tx,
            &wisp_store::HotMeta {
                ref_name: ref_name.to_string(),
                head: head.unwrap_or(ChangesetId::from_bytes([0u8; 16])),
                ledger_seq: max_seq,
                profile: profile_id,
            },
        )?;
        tx.commit()
            .map_err(|e| WispError::internal(e.to_string()))?;
        drop(ltx);
        Ok(())
    })();
    repo.hots.insert(ref_name.to_string(), hot);
    result
}
