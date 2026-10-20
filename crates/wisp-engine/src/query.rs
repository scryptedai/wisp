//! Read commands and maintenance: log, show, why, search, pack, tokens, fsck, hot, branch/switch,
//! mv, lifecycle, verify, revert, validate, backup, gc, migrate, feed, actors, codecs, config.

use crate::apply::{self, history_chain};
use crate::checkout;
use crate::commit::{commit, CommitRequest, PlannedOp, PolicyFacts, Precondition};
use crate::erase::resolve_target;
use crate::payloads::*;
use crate::repo::{Repository, MAIN_REF};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use wisp_model::*;
use wisp_store::{Hot, Ledger};

// ---- tokens ----

/// The approximate tokenizer codec used in this build (DECISIONS.md §14.3).
pub const TOKEN_CODEC: &str = "tok:approx:chars4";

pub fn count_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    (chars + 3) / 4
}

// ---- log / show / why ----

fn change_summary(repo: &Repository, c: &Change, conn: Option<&rusqlite::Connection>) -> Value {
    let mut v =
        json!({ "id": c.id.to_string(), "op": c.op.as_str(), "scope": c.scope.to_string() });
    if let Some(cp) = c.target.concept() {
        v["concept"] = json!(cp.to_string());
        if let Some(conn) = conn {
            if let Ok(Some(st)) = Hot::concept(conn, cp) {
                v["path"] = json!(st.path);
            } else if let Ok(Some(r)) = Hot::retracted(conn, cp) {
                v["path"] = json!(r.0);
            }
        }
    }
    if let Some(s) = c.target.section() {
        v["section"] = json!(s.to_string());
    }
    if let Target::Erase { scopes } = &c.target {
        v["scopes"] = json!(scopes.len());
    }
    if !c.inputs.is_empty() {
        v["inputs"] = serde_json::to_value(&c.inputs).unwrap_or(Value::Null);
    }
    if let Some(l) = &c.lineage {
        v["lineage"] = serde_json::to_value(l).unwrap_or(Value::Null);
    }
    if let Some(r) = c.reason {
        v["reason"] = json!(r.as_str());
    }
    if let Some(o) = &c.origin {
        v["origin"] = json!({ "actor": repo.actor_display(o.actor_id), "at": o.at });
    }
    v
}

pub struct LogFilter<'a> {
    pub path: Option<&'a str>,
    pub ops: Option<Vec<Op>>,
    pub actor: Option<&'a str>,
    pub since: Option<&'a str>,
    pub include_erased: bool,
    pub limit: Option<usize>,
}

pub fn log(repo: &mut Repository, ref_name: &str, f: &LogFilter<'_>) -> Result<Value> {
    let head = repo.head(ref_name)?;
    let chain = history_chain(&repo.ledger, head)?;
    let concept_filter = match f.path {
        Some(p) => Some(resolve_target(repo, ref_name, p)?.0.concept),
        None => None,
    };
    repo.hot(ref_name)?;
    let hot = repo.hots.remove(ref_name).expect("hot");
    let mut out = Vec::new();
    for row in chain.iter().rev() {
        let cs = &row.changeset;
        if let Some(s) = f.since {
            if cs.at.as_str() < s {
                continue;
            }
        }
        let actor = repo.actor_display(cs.actor_id);
        if let Some(a) = f.actor {
            if actor != a {
                continue;
            }
        }
        let changes = Ledger::changes_of(repo.ledger.conn(), cs.id)?;
        let filtered: Vec<&Change> = changes
            .iter()
            .filter(|c| {
                concept_filter
                    .map(|cf| c.target.concept() == Some(cf))
                    .unwrap_or(true)
            })
            .filter(|c| {
                f.ops
                    .as_ref()
                    .map(|ops| ops.contains(&c.op))
                    .unwrap_or(true)
            })
            .filter(|c| f.include_erased || c.op != Op::Erase || concept_filter.is_none())
            .collect();
        if filtered.is_empty() && (concept_filter.is_some() || f.ops.is_some()) {
            continue;
        }
        let message = match &cs.message {
            Some(m) => repo.read_payload_string(repo.repo_scope(), m)?,
            None => None,
        };
        out.push(json!({
            "changeset": cs.id.to_string(),
            "seq": row.seq,
            "at": cs.at,
            "actor": actor,
            "parents": cs.parents.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
            "based_on": cs.based_on.map(|b| b.to_string()),
            "message": message,
            "changes": filtered.iter().map(|c| change_summary(repo, c, Some(hot.conn()))).collect::<Vec<_>>(),
        }));
        if let Some(l) = f.limit {
            if out.len() >= l {
                break;
            }
        }
    }
    repo.hots.insert(ref_name.to_string(), hot);
    Ok(json!({ "ref": ref_name, "head": head.map(|h| h.to_string()), "changesets": out }))
}

pub fn show(repo: &mut Repository, ref_name: &str, cs_id: ChangesetId) -> Result<Value> {
    let row = Ledger::get_changeset(repo.ledger.conn(), cs_id)?
        .ok_or_else(|| WispError::not_found(format!("no changeset {cs_id}")))?;
    let cs = &row.changeset;
    let message = match &cs.message {
        Some(m) => repo.read_payload_string(repo.repo_scope(), m)?,
        None => None,
    };
    repo.hot(ref_name)?;
    let hot = repo.hots.remove(ref_name).expect("hot");
    let mut changes = Vec::new();
    for c in Ledger::changes_of(repo.ledger.conn(), cs.id)? {
        let mut v = change_summary(repo, &c, Some(hot.conn()));
        if let Some(n) = &c.note {
            v["note"] = json!(repo.read_payload_string(c.scope, n)?);
        }
        if let Some(p) = &c.payload {
            if matches!(
                c.op,
                Op::AddSection | Op::UpdateSection | Op::Split | Op::MoveSection
            ) {
                if let Some(pt) = repo.read_payload(c.scope, p)? {
                    let sb = SectionBody::decode(&pt)?;
                    v["heading"] = json!(sb.heading);
                    v["content"] = json!(sb.body);
                } else {
                    v["content"] = json!(null);
                    v["erased"] = json!(true);
                }
            }
        }
        changes.push(v);
    }
    repo.hots.insert(ref_name.to_string(), hot);
    Ok(json!({
        "changeset": cs.id.to_string(), "seq": row.seq, "at": cs.at, "actor": repo.actor_display(cs.actor_id),
        "parents": cs.parents.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
        "based_on": cs.based_on.map(|b| b.to_string()),
        "digest": hex::encode(cs.digest), "signed": cs.sig.is_some(), "message": message, "changes": changes
    }))
}

/// `wisp why <path>[#section] --depth n`: content history, actors, origins, and the input graph.
pub fn why(repo: &mut Repository, ref_name: &str, spec: &str, depth: usize) -> Result<Value> {
    let (c, section) = resolve_target(repo, ref_name, spec)?;
    fn history(
        repo: &Repository,
        concept: ConceptId,
        section: Option<SectionId>,
        depth: usize,
        seen: &mut Vec<ConceptId>,
    ) -> Result<Value> {
        let rows = Ledger::changes_for_concept(repo.ledger.conn(), concept, section)?;
        let mut events = Vec::new();
        let mut inputs_graph = Vec::new();
        for (seq, ch) in rows {
            let Some(row) = Ledger::get_changeset(repo.ledger.conn(), ch.changeset)? else {
                continue;
            };
            let mut e = json!({
                "seq": seq, "changeset": ch.changeset.to_string(), "change": ch.id.to_string(),
                "at": row.changeset.at, "actor": repo.actor_display(row.changeset.actor_id), "op": ch.op.as_str(),
            });
            if let Some(s) = ch.target.section() {
                e["section"] = json!(s.to_string());
            }
            if let Some(o) = &ch.origin {
                e["origin"] = json!({ "actor": repo.actor_display(o.actor_id), "at": o.at });
            }
            if let Some(l) = &ch.lineage {
                e["lineage"] = serde_json::to_value(l).unwrap_or(Value::Null);
            }
            if !ch.inputs.is_empty() {
                e["inputs"] = serde_json::to_value(&ch.inputs).unwrap_or(Value::Null);
                if depth > 0 {
                    for i in &ch.inputs {
                        if let InputRef::Concept {
                            concept: ic,
                            section: is,
                            ..
                        } = i
                        {
                            if !seen.contains(ic) {
                                seen.push(*ic);
                                inputs_graph.push(json!({ "concept": ic.to_string(), "section": is.map(|s| s.to_string()), "history": history(repo, *ic, *is, depth - 1, seen)? }));
                            }
                        }
                    }
                }
            }
            if let Some(n) = &ch.note {
                e["note"] = json!(repo.read_payload_string(ch.scope, n)?);
            }
            events.push(e);
        }
        Ok(json!({ "events": events, "inputs": inputs_graph }))
    }
    let mut seen = vec![c.concept];
    let h = history(repo, c.concept, section, depth, &mut seen)?;
    Ok(
        json!({ "path": c.path, "concept": c.concept.to_string(), "section": section.map(|s| s.to_string()), "history": h }),
    )
}

// ---- search / pack / tokens ----

pub fn search(
    repo: &mut Repository,
    ref_name: &str,
    query: &str,
    mode: &str,
    k: usize,
) -> Result<Value> {
    crate::search::execute(
        repo,
        ref_name,
        query,
        mode,
        k,
        &crate::search::SearchParams::default(),
    )
}

pub fn search_with_params(
    repo: &mut Repository,
    ref_name: &str,
    query: &str,
    mode: &str,
    k: usize,
    params: &crate::search::SearchParams,
) -> Result<Value> {
    crate::search::execute(repo, ref_name, query, mode, k, params)
}

fn search_vector_hit_json(h: &wisp_index::VectorSearchHit) -> Value {
    json!({
        "concept": h.concept.to_string(),
        "path": h.path,
        "section": h.section.to_string(),
        "heading": h.heading,
        "score": h.score,
        "tokens": count_tokens(&h.body),
    })
}

fn search_hit_json(h: &wisp_store::SearchHit) -> Value {
    json!({
        "concept": h.concept.to_string(),
        "path": h.path,
        "section": h.section.to_string(),
        "heading": h.heading,
        "score": h.score,
        "tokens": count_tokens(&h.body),
    })
}

fn hybrid_merge(
    kw: Vec<wisp_store::SearchHit>,
    vec: &[wisp_index::VectorSearchHit],
    k: usize,
) -> Vec<Value> {
    use std::collections::HashMap;
    let mut scores: HashMap<String, (f64, wisp_store::SearchHit)> = HashMap::new();
    for (i, h) in kw.into_iter().enumerate() {
        let key = h.section.to_string();
        let rrf = 1.0 / (60.0 + i as f64);
        scores.insert(key, (rrf, h));
    }
    for (i, v) in vec.iter().enumerate() {
        let key = v.section.to_string();
        let rrf = 1.0 / (60.0 + i as f64);
        if let Some((s, _)) = scores.get_mut(&key) {
            *s += rrf;
        } else {
            scores.insert(
                key,
                (
                    rrf,
                    wisp_store::SearchHit {
                        concept: v.concept,
                        section: v.section,
                        path: v.path.clone(),
                        heading: v.heading.clone(),
                        score: v.score as f64,
                        body: v.body.clone(),
                    },
                ),
            );
        }
    }
    let mut ranked: Vec<_> = scores.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1 .0
            .partial_cmp(&a.1 .0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked
        .into_iter()
        .take(k)
        .map(|(_, (score, h))| {
            let mut o = search_hit_json(&h);
            o.as_object_mut()
                .unwrap()
                .insert("score".into(), json!(score));
            o
        })
        .collect()
}

/// `wisp pack [<path>...] [--budget <tokens>]` (§14.5).
pub fn pack(
    repo: &mut Repository,
    ref_name: &str,
    paths: &[String],
    budget: Option<usize>,
) -> Result<Value> {
    let targets: Vec<(wisp_store::ConceptState, Option<SectionId>)> = if paths.is_empty() {
        let hot = repo.hot(ref_name)?;
        Hot::concepts(hot.conn())?
            .into_iter()
            .map(|c| (c, None))
            .collect()
    } else {
        let mut v = Vec::new();
        for p in paths {
            v.push(resolve_target(repo, ref_name, p)?);
        }
        v
    };
    let okf_version = repo.profile.okf_version().to_string();
    let hot = repo.hot(ref_name)?;
    let conn = hot.conn();
    let mut concepts = Vec::new();
    let mut omitted = Vec::new();
    let mut used = 0usize;
    for (c, only_section) in targets {
        let fm: BTreeMap<String, Value> = Hot::frontmatter_of(conn, c.concept)?
            .into_iter()
            .filter_map(|e| {
                wisp_parse::frontmatter::entry_value(&e.raw)
                    .and_then(|v| serde_json::to_value(v).ok())
                    .map(|v| (e.key, v))
            })
            .collect();
        let sections_all = Hot::sections_of(conn, c.concept)?;
        let sections: Vec<&wisp_store::SectionState> = sections_all
            .iter()
            .filter(|s| only_section.map(|o| o == s.section).unwrap_or(true))
            .collect();
        let total: usize = sections
            .iter()
            .map(|s| count_tokens(&s.heading) + count_tokens(&s.body))
            .sum();
        let mut included = Vec::new();
        let whole_fits = budget.map(|b| used + total <= b).unwrap_or(true);
        if whole_fits {
            for s in &sections {
                included.push(*s);
            }
            used += total;
        } else {
            // Whole concepts first; then sections in order until the budget is reached.
            for s in &sections {
                let t = count_tokens(&s.heading) + count_tokens(&s.body);
                if budget.map(|b| used + t <= b).unwrap_or(true) {
                    included.push(*s);
                    used += t;
                } else {
                    omitted.push(json!({ "path": c.path, "section": s.section.to_string(), "heading": s.heading, "tokens": t }));
                }
            }
        }
        if included.is_empty() {
            continue;
        }
        let links: Vec<Value> = {
            let mut st = conn
                .prepare("SELECT section, target_concept, target_path FROM state_links WHERE concept = ?1")
                .map_err(|e| WispError::internal(e.to_string()))?;
            let rows = st
                .query_map(rusqlite::params![c.concept.bytes().to_vec()], |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, Option<Vec<u8>>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|e| WispError::internal(e.to_string()))?
                .filter_map(|r| r.ok());
            let mut out = Vec::new();
            for (s, t, p) in rows {
                let section = SectionId::from_slice(&s)
                    .map(|x| x.to_string())
                    .unwrap_or_default();
                let target = t.and_then(|b| ConceptId::from_slice(&b));
                let path = match target {
                    Some(tc) => Hot::concept(conn, tc)?.map(|x| x.path),
                    None => p,
                };
                out.push(json!({ "section": section, "target": target.map(|x| x.to_string()), "path": path }));
            }
            out
        };
        concepts.push(json!({
            "wisp_id": c.concept.to_string(),
            "path": c.path,
            "frontmatter": fm,
            "tokens": included.iter().map(|s| count_tokens(&s.heading) + count_tokens(&s.body)).sum::<usize>(),
            "sections": included.iter().map(|s| json!({ "id": s.section.to_string(), "heading": s.heading, "level": s.level, "tokens": count_tokens(&s.heading) + count_tokens(&s.body), "text": s.body })).collect::<Vec<_>>(),
            "links": links,
        }));
    }
    Ok(
        json!({ "okf_version": okf_version, "codec": TOKEN_CODEC, "tokens": used, "budget": budget, "concepts": concepts, "omitted": omitted }),
    )
}

pub fn tokens(repo: &mut Repository, ref_name: &str, spec: &str) -> Result<Value> {
    let (c, section) = resolve_target(repo, ref_name, spec)?;
    let hot = repo.hot(ref_name)?;
    let sections = Hot::sections_of(hot.conn(), c.concept)?;
    let per: Vec<Value> = sections
        .iter()
        .filter(|s| section.map(|x| x == s.section).unwrap_or(true))
        .map(|s| json!({ "section": s.section.to_string(), "heading": s.heading, "tokens": count_tokens(&s.heading) + count_tokens(&s.body) }))
        .collect();
    let total: usize = per
        .iter()
        .map(|p| p["tokens"].as_u64().unwrap_or(0) as usize)
        .sum();
    Ok(json!({ "path": c.path, "codec": TOKEN_CODEC, "tokens": total, "sections": per }))
}

// ---- fsck / hot ----

/// Logical dump of a hot tier for comparison (I13, `hot check`).
pub fn hot_dump(conn: &rusqlite::Connection) -> Result<Value> {
    let mut concepts = Vec::new();
    for c in Hot::concepts(conn)? {
        concepts.push(json!({
            "concept": c.concept.to_string(), "path": c.path, "type": c.type_, "status": c.status.map(|s| s.as_str()),
            "version": c.version.to_string(), "content_version": c.content_version.map(|v| v.to_string()),
            "sections": Hot::sections_of(conn, c.concept)?.into_iter().map(|s| json!([s.section.to_string(), s.level, s.heading, s.heading_line, s.body, s.version.to_string()])).collect::<Vec<_>>(),
            "frontmatter": Hot::frontmatter_of(conn, c.concept)?.into_iter().map(|e| json!([e.key, e.raw, e.version.to_string()])).collect::<Vec<_>>(),
            "sources": Hot::sources_of(conn, c.concept)?.into_iter().map(|s| json!([s.source_id, s.record, s.version.to_string()])).collect::<Vec<_>>(),
        }));
    }
    let assets: Vec<Value> = Hot::assets(conn)?
        .into_iter()
        .map(|a| {
            json!([
                a.asset.to_string(),
                a.path,
                a.bytes.len(),
                a.version.to_string()
            ])
        })
        .collect();
    let retracted: Vec<Value> = Hot::retracted_all(conn)?
        .into_iter()
        .map(|(c, p)| json!([c.to_string(), p]))
        .collect();
    Ok(json!({ "concepts": concepts, "assets": assets, "retracted": retracted }))
}

/// `wisp hot check`: compare the incrementally maintained hot tier with a fresh rebuild (exit 8 on mismatch).
pub fn hot_check(repo: &mut Repository, ref_name: &str) -> Result<Value> {
    let current = {
        let hot = repo.hot(ref_name)?;
        hot_dump(hot.conn())?
    };
    // Rebuild into a scratch tier.
    let scratch_dir = repo
        .wisp_dir
        .join("cache")
        .join(format!("check-{}", ChangeId::new()));
    std::fs::create_dir_all(&scratch_dir)?;
    let mut scratch = Hot::open(&scratch_dir.join("hot.db"))?;
    let head = repo.head(ref_name)?;
    let chain = history_chain(&repo.ledger, head)?;
    let skip = head.and_then(|h| crate::merge::merge_skips_for_head(repo, ref_name, h));
    {
        let reader = repo.reader();
        let tx = scratch.begin()?;
        for row in &chain {
            let changes = Ledger::changes_of(reader.conn, row.changeset.id)?;
            let changes: Vec<wisp_model::Change> = match &skip {
                Some(skip) => changes
                    .into_iter()
                    .filter(|c| !skip.contains(&c.id))
                    .collect(),
                None => changes,
            };
            apply::apply_changeset(&reader, &tx, &row.changeset, &changes)?;
        }
        tx.commit()
            .map_err(|e| WispError::internal(e.to_string()))?;
    }
    let rebuilt = hot_dump(scratch.conn())?;
    drop(scratch);
    let _ = std::fs::remove_dir_all(&scratch_dir);
    if current == rebuilt {
        Ok(json!({ "ref": ref_name, "ok": true }))
    } else {
        Err(WispError::integrity("hot tier does not match the ledger")
            .with_details(json!({ "ref": ref_name })))
    }
}

pub fn hot_rebuild(repo: &mut Repository, ref_name: &str) -> Result<Value> {
    repo.drop_hot(ref_name);
    let _ = std::fs::remove_file(repo.hot_path(ref_name));
    let _ = std::fs::remove_file(repo.hot_path(ref_name).with_extension("db-wal"));
    let _ = std::fs::remove_file(repo.hot_path(ref_name).with_extension("db-shm"));
    repo.hot(ref_name)?;
    Ok(json!({ "ref": ref_name, "rebuilt": true }))
}

/// `wisp fsck` (§10.3): digests, signatures, commitments, orphaned payloads, shredded tombstones, hot tiers.
pub fn fsck(repo: &mut Repository) -> Result<Value> {
    let mut problems: Vec<Value> = Vec::new();
    let mut checked = 0usize;
    // Digest chain.
    let rows = Ledger::changesets(repo.ledger.conn())?;
    let digests: BTreeMap<ChangesetId, [u8; 32]> = rows
        .iter()
        .map(|r| (r.changeset.id, r.changeset.digest))
        .collect();
    for row in &rows {
        let cs = &row.changeset;
        let mut parent_digests = Vec::new();
        for p in &cs.parents {
            match digests.get(p) {
                Some(d) => parent_digests.push(*d),
                None => problems.push(json!({ "kind": "missing_parent", "changeset": cs.id.to_string(), "parent": p.to_string() })),
            }
        }
        let changes = Ledger::changes_of(repo.ledger.conn(), cs.id)?;
        let d = canonical::changeset_digest(
            cs.id,
            cs.actor_id,
            &cs.at,
            &parent_digests,
            cs.based_on,
            cs.message.as_ref(),
            &changes,
        );
        if d != cs.digest {
            problems.push(json!({ "kind": "digest_mismatch", "changeset": cs.id.to_string() }));
        }
        if let Some(sig) = &cs.sig {
            if let Some(actor) = Ledger::actor(repo.ledger.conn(), cs.actor_id)? {
                match actor
                    .public_key
                    .and_then(|k| repo.read_payload(actor.scope, &k).ok().flatten())
                {
                    Some(pk) => {
                        if !wisp_crypto::sig::verify(&pk, &cs.digest, sig) {
                            problems.push(
                                json!({ "kind": "bad_signature", "changeset": cs.id.to_string() }),
                            );
                        }
                    }
                    None => { /* actor erased: signature can no longer be checked; digest still verifies */
                    }
                }
            }
        }
        checked += 1;
    }
    // Payloads: present ones decrypt and match their commitments; absent ones have no file and no key.
    let mut payload_checked = 0usize;
    let commits: BTreeMap<PayloadId, (ScopeId, Commit)> = Ledger::changes_all(repo.ledger.conn())?
        .into_iter()
        .flat_map(|(_, c)| {
            let mut v = Vec::new();
            if let Some(p) = c.payload {
                v.push((p.id, (c.scope, p.commit)));
            }
            if let Some(n) = c.note {
                v.push((n.id, (c.scope, n.commit)));
            }
            v
        })
        .collect();
    for (id, scope, present) in Ledger::all_payloads(repo.ledger.conn())? {
        let on_disk = repo.ledger.read_payload(id)?;
        if present {
            match on_disk {
                None => {
                    problems.push(json!({ "kind": "payload_missing", "payload": id.to_string() }))
                }
                Some(_) => {
                    if let Some((s, commit)) = commits.get(&id) {
                        match repo.read_payload(*s, &PayloadRef { id, commit: *commit }) {
                            Ok(Some(_)) => payload_checked += 1,
                            Ok(None) => problems.push(json!({ "kind": "payload_key_missing", "payload": id.to_string() })),
                            Err(e) => problems.push(json!({ "kind": "payload_corrupt", "payload": id.to_string(), "error": e.message })),
                        }
                    }
                }
            }
        } else {
            if on_disk.is_some() {
                problems.push(json!({ "kind": "erased_payload_present", "payload": id.to_string(), "scope": scope.to_string() }));
            }
        }
    }
    // Tombstoned scopes fully shredded.
    for s in Ledger::all_scopes(repo.ledger.conn())? {
        if s.erased {
            if !s.shredded {
                problems.push(json!({ "kind": "scope_not_shredded", "scope": s.id.to_string() }));
            }
            if repo.keystore.has(s.id)? {
                problems.push(json!({ "kind": "erased_scope_has_key", "scope": s.id.to_string() }));
            }
        }
    }
    // Hot tiers match the ledger.
    let mut hot_results = Vec::new();
    for (name, _) in Ledger::refs(repo.ledger.conn())? {
        if repo.hot_path(&name).exists() {
            match hot_check(repo, &name) {
                Ok(_) => hot_results.push(json!({ "ref": name, "ok": true })),
                Err(e) => {
                    hot_results.push(json!({ "ref": name, "ok": false }));
                    problems
                        .push(json!({ "kind": "hot_mismatch", "ref": name, "error": e.message }));
                }
            }
        }
    }
    let ok = problems.is_empty();
    let v = json!({ "ok": ok, "changesets": checked, "payloads": payload_checked, "hot": hot_results, "problems": problems });
    if ok {
        Ok(v)
    } else {
        Err(WispError::integrity("fsck found problems").with_details(v))
    }
}

// ---- refs ----

pub fn branch(repo: &mut Repository, name: &str, from: Option<&str>) -> Result<Value> {
    if Ledger::head(repo.ledger.conn(), name)?.is_some() {
        return Err(WispError::usage(format!("ref {name} exists")));
    }
    let src = from.unwrap_or(MAIN_REF);
    let head = repo
        .head(src)?
        .ok_or_else(|| WispError::usage(format!("ref {src} has no commits yet")))?;
    let tx = repo.ledger.begin_immediate()?;
    Ledger::set_head(&tx, name, head)?;
    tx.commit()
        .map_err(|e| WispError::internal(e.to_string()))?;
    Ok(json!({ "ref": name, "head": head.to_string(), "from": src }))
}

pub fn refs(repo: &Repository) -> Result<Value> {
    Ok(json!(Ledger::refs(repo.ledger.conn())?
        .into_iter()
        .map(|(n, h)| json!({ "ref": n, "head": h.to_string() }))
        .collect::<Vec<_>>()))
}

/// `wisp switch <ref>`: rebuild the hot tier and working tree for that ref (exit 3 with unrecorded edits unless --discard).
pub fn switch(repo: &mut Repository, name: &str, discard: bool) -> Result<Value> {
    if Ledger::head(repo.ledger.conn(), name)?.is_none() {
        return Err(WispError::not_found(format!("no ref {name}")));
    }
    let st = crate::record::status(repo, None)?;
    if !discard && (!st.changed.is_empty() || !st.new.is_empty() || !st.missing.is_empty()) {
        return Err(WispError::conflict(
            "unrecorded edits; pass --discard to drop them",
            json!([{ "type": "unrecorded_edits", "changed": st.changed, "new": st.new, "missing": st.missing }]),
        ));
    }
    let root = repo.root.clone();
    let idx = checkout::write_tree(repo, name, &root, false, false, true)?;
    idx.save(&checkout::checkout_idx_path(repo, &root))?;
    Ok(json!({ "ref": name, "head": idx.head.map(|h| h.to_string()) }))
}

/// The ref checked out in the main working tree.
pub fn current_ref(repo: &Repository) -> String {
    checkout::CheckoutIndex::load(&repo.wisp_dir.join("checkout.idx"))
        .ok()
        .flatten()
        .map(|i| i.ref_name)
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| MAIN_REF.to_string())
}

// ---- mv / lifecycle / verify / revert ----

fn relative_path(from_dir: &str, to: &str) -> String {
    let from: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let to_parts: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let mut i = 0;
    while i < from.len() && i < to_parts.len() && from[i] == to_parts[i] {
        i += 1;
    }
    let mut out: Vec<String> = vec!["..".to_string(); from.len() - i];
    out.extend(to_parts[i..].iter().map(|s| s.to_string()));
    out.join("/")
}

/// `wisp mv <from> <to>`: move plus link rewrites, one changeset (§13.1).
pub fn mv(
    repo: &mut Repository,
    ref_name: &str,
    from: &str,
    to: &str,
    actor: &str,
) -> Result<Value> {
    let (c, _) = resolve_target(repo, ref_name, from)?;
    let new_path = concept_path_of(to);
    let old_path = c.path.clone();
    let mut ops = Vec::new();
    let mut mop = PlannedOp::new(Op::Move, Target::Concept { concept: c.concept }, c.scope)
        .with_payload(PayloadKind::Path, new_path.as_bytes().to_vec())
        .with_precondition(Precondition::PathFree(new_path.clone()))
        .with_path(format!("{new_path}.md"));
    mop.policy = PolicyFacts {
        concept_type: c.type_.clone(),
        ..Default::default()
    };
    ops.push(mop);
    // Rewrite links in every section that points at the old path, preserving link form.
    let backlinks = {
        let hot = repo.hot(ref_name)?;
        Hot::backlinks(hot.conn(), c.concept)?
    };
    let mut rewritten = Vec::new();
    for (lc, ls) in backlinks {
        let hot = repo.hot(ref_name)?;
        let Some(sec) = Hot::section(hot.conn(), ls)? else {
            continue;
        };
        let Some(lcs) = Hot::concept(hot.conn(), lc)? else {
            continue;
        };
        let parsed = wisp_parse::sections::section(&sec.body);
        let mut body = sec.body.clone();
        for ps in &parsed.sections {
            for l in &ps.links {
                if l.form == wisp_parse::LinkForm::External {
                    continue;
                }
                let Some(resolved) = apply::resolve_link_path(&lcs.path, &l.dest, &l.form) else {
                    continue;
                };
                if resolved != old_path {
                    continue;
                }
                let (dest_path, suffix) = match l.dest.find('#') {
                    Some(i) => (&l.dest[..i], &l.dest[i..]),
                    None => (l.dest.as_str(), ""),
                };
                let has_md = dest_path.ends_with(".md");
                let new_dest = match l.form {
                    wisp_parse::LinkForm::Absolute => {
                        format!("/{}{}{}", new_path, if has_md { ".md" } else { "" }, suffix)
                    }
                    _ => {
                        let dir = lcs.path.rfind('/').map(|i| &lcs.path[..i]).unwrap_or("");
                        format!(
                            "{}{}{}",
                            relative_path(dir, &new_path),
                            if has_md { ".md" } else { "" },
                            suffix
                        )
                    }
                };
                body = body.replace(&format!("]({})", l.dest), &format!("]({})", new_dest));
                body = body.replace(&format!("]: {}", l.dest), &format!("]: {}", new_dest));
            }
        }
        if body != sec.body {
            let sb = SectionBody {
                level: sec.level,
                heading: sec.heading.clone(),
                heading_line: sec.heading_line.clone(),
                body,
                after: None,
                at_start: false,
            };
            let mut op = PlannedOp::new(
                Op::UpdateSection,
                Target::Section {
                    concept: lc,
                    section: ls,
                },
                sec.scope,
            )
            .with_payload(PayloadKind::Section, sb.encode())
            .with_precondition(Precondition::SectionVersion {
                section: ls,
                version: sec.version,
            })
            .with_path(format!("{}.md", lcs.path));
            op.policy = PolicyFacts {
                concept_type: lcs.type_.clone(),
                section_heading: Some(sec.heading.clone()),
                ..Default::default()
            };
            ops.push(op);
            rewritten.push(json!({ "path": lcs.path, "section": ls.to_string() }));
        }
    }
    let mut req = CommitRequest::new(ref_name, actor);
    req.message = Some(format!("Move {old_path} to {new_path}"));
    req.ops = ops;
    let out = commit(repo, req)?;
    // Working tree: remove the old file, render the new one.
    if ref_name == MAIN_REF {
        let old_file = repo.root.join(format!("{old_path}.md"));
        let _ = std::fs::remove_file(old_file);
        let root = repo.root.clone();
        let idx = checkout::write_tree(repo, ref_name, &root, false, false, true)?;
        idx.save(&checkout::checkout_idx_path(repo, &root))?;
    }
    Ok(
        json!({ "changeset": out.changeset.to_string(), "from": old_path, "to": new_path, "links_rewritten": rewritten }),
    )
}

/// `wisp revert <changeset>`: record inverse operations. Cannot revert an erase (§16).
pub fn revert(
    repo: &mut Repository,
    ref_name: &str,
    cs_id: ChangesetId,
    actor: &str,
) -> Result<Value> {
    let changes = Ledger::changes_of(repo.ledger.conn(), cs_id)?;
    if changes.is_empty() {
        return Err(WispError::not_found(format!("no changeset {cs_id}")));
    }
    if changes.iter().any(|c| c.op == Op::Erase) {
        return Err(WispError::refused("cannot revert an erase"));
    }
    let mut ops = Vec::new();
    let mut skipped = Vec::new();
    for c in changes.iter().rev() {
        match (&c.op, &c.target) {
            (Op::UpdateSection, Target::Section { concept, section }) | (Op::AddSection, Target::Section { concept, section }) | (Op::Split, Target::Section { concept, section }) => {
                let hot = repo.hot(ref_name)?;
                let Some(cur) = Hot::section(hot.conn(), *section)? else { skipped.push(json!({ "change": c.id.to_string(), "why": "section gone" })); continue };
                // The previous content version of this section before `c`.
                let hist = Ledger::changes_for_concept(repo.ledger.conn(), *concept, Some(*section))?;
                let prev = hist.iter().take_while(|(_, h)| h.id != c.id).filter(|(_, h)| matches!(h.op, Op::AddSection | Op::UpdateSection | Op::Split | Op::MoveSection)).last().map(|(_, h)| h.clone());
                match (c.op, prev) {
                    (Op::UpdateSection, Some(p)) => {
                        let Some(pr) = &p.payload else { continue };
                        let Some(pt) = repo.read_payload(p.scope, pr)? else { skipped.push(json!({ "change": c.id.to_string(), "why": "previous content erased" })); continue };
                        let mut sb = SectionBody::decode(&pt)?;
                        sb.after = None;
                        ops.push(
                            PlannedOp::new(Op::UpdateSection, Target::Section { concept: *concept, section: *section }, cur.scope)
                                .with_payload(PayloadKind::Section, sb.encode())
                                .with_precondition(Precondition::SectionVersion { section: *section, version: cur.version }),
                        );
                    }
                    (Op::AddSection, _) | (Op::Split, _) => {
                        ops.push(
                            PlannedOp::new(Op::RemoveSection, Target::Section { concept: *concept, section: *section }, cur.scope)
                                .with_precondition(Precondition::SectionVersion { section: *section, version: cur.version }),
                        );
                    }
                    _ => skipped.push(json!({ "change": c.id.to_string(), "why": "no previous version" })),
                }
            }
            (Op::RemoveSection, Target::Section { concept, section }) => {
                let hist = Ledger::changes_for_concept(repo.ledger.conn(), *concept, Some(*section))?;
                let prev = hist.iter().take_while(|(_, h)| h.id != c.id).filter(|(_, h)| matches!(h.op, Op::AddSection | Op::UpdateSection | Op::Split | Op::MoveSection)).last().map(|(_, h)| h.clone());
                let Some(p) = prev else { continue };
                let Some(pr) = &p.payload else { continue };
                let Some(pt) = repo.read_payload(p.scope, pr)? else { skipped.push(json!({ "change": c.id.to_string(), "why": "content erased" })); continue };
                let sb = SectionBody::decode(&pt)?;
                let new_id = SectionId::new();
                let sscope = ScopeId::new();
                ops.push(
                    PlannedOp::new(Op::AddSection, Target::Section { concept: *concept, section: new_id }, sscope)
                        .with_new_scope(ScopeKind::Section { concept: *concept, section: new_id })
                        .with_payload(PayloadKind::Section, sb.encode())
                        .with_precondition(Precondition::ConceptLive(*concept)),
                );
            }
            (Op::SetKey, Target::FrontmatterKey { concept, key_commit }) | (Op::UnsetKey, Target::FrontmatterKey { concept, key_commit }) => {
                let Some(pr) = &c.payload else { continue };
                let Some(pt) = repo.read_payload(c.scope, pr)? else { continue };
                let fm = decode_frontmatter(&pt)?;
                let hist = Ledger::changes_for_concept(repo.ledger.conn(), *concept, None)?;
                let prev = hist
                    .iter()
                    .take_while(|(_, h)| h.id != c.id)
                    .filter(|(_, h)| matches!(h.op, Op::SetKey | Op::UnsetKey) && matches!(&h.target, Target::FrontmatterKey { key_commit: kc, .. } if kc == key_commit))
                    .last()
                    .map(|(_, h)| h.clone());
                let hot = repo.hot(ref_name)?;
                let cur = Hot::frontmatter_entry(hot.conn(), *concept, &fm.key)?.map(|e| e.version);
                let pre = Precondition::EntryVersion { concept: *concept, key: fm.key.clone(), version: cur };
                match prev {
                    Some(p) if p.op == Op::SetKey => {
                        let Some(ppr) = &p.payload else { continue };
                        let Some(ppt) = repo.read_payload(p.scope, ppr)? else { continue };
                        ops.push(PlannedOp::new(Op::SetKey, c.target.clone(), c.scope).with_payload(PayloadKind::FrontmatterEntry, ppt).with_precondition(pre));
                    }
                    _ => {
                        if c.op == Op::SetKey {
                            ops.push(PlannedOp::new(Op::UnsetKey, c.target.clone(), c.scope).with_payload(PayloadKind::FrontmatterEntry, encode_frontmatter(&fm.key, "")).with_precondition(pre));
                        }
                    }
                }
            }
            (Op::Retract, Target::Concept { concept }) => {
                ops.push(PlannedOp::new(Op::Restore, Target::Concept { concept: *concept }, c.scope).with_precondition(Precondition::ConceptRetracted(*concept)));
            }
            (Op::Restore, Target::Concept { concept }) => {
                let mut op = PlannedOp::new(Op::Retract, Target::Concept { concept: *concept }, c.scope).with_precondition(Precondition::ConceptLive(*concept));
                op.reason = Some(Reason::Withdrawn);
                ops.push(op);
            }
            (Op::SetStatus, Target::Concept { concept }) => {
                let hist = Ledger::changes_for_concept(repo.ledger.conn(), *concept, None)?;
                let prev = hist.iter().take_while(|(_, h)| h.id != c.id).filter(|(_, h)| h.op == Op::SetStatus).last().map(|(_, h)| h.clone());
                let status = match prev {
                    Some(p) => p.payload.and_then(|pr| repo.read_payload(p.scope, &pr).ok().flatten()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_else(|| "stable".into()),
                    None => "stable".into(),
                };
                ops.push(PlannedOp::new(Op::SetStatus, Target::Concept { concept: *concept }, c.scope).with_payload(PayloadKind::Status, status.into_bytes()).with_precondition(Precondition::ConceptLive(*concept)));
            }
            _ => skipped.push(json!({ "change": c.id.to_string(), "op": c.op.as_str(), "why": "not invertible in this build" })),
        }
    }
    if ops.is_empty() {
        return Err(WispError::usage("nothing invertible in that changeset")
            .with_details(json!({ "skipped": skipped })));
    }
    let mut req = CommitRequest::new(ref_name, actor);
    req.message = Some(format!("Revert {cs_id}"));
    req.ops = ops;
    let out = commit(repo, req)?;
    if ref_name == MAIN_REF {
        let root = repo.root.clone();
        let idx = checkout::write_tree(repo, ref_name, &root, false, false, true)?;
        idx.save(&checkout::checkout_idx_path(repo, &root))?;
    }
    Ok(
        json!({ "changeset": out.changeset.to_string(), "reverted": cs_id.to_string(), "skipped": skipped }),
    )
}

// ---- validate / migrate ----

pub fn validate(
    repo: &mut Repository,
    ref_name: &str,
    path: Option<&str>,
    strict: bool,
) -> Result<Value> {
    let targets = match path {
        Some(p) => vec![resolve_target(repo, ref_name, p)?.0],
        None => {
            let hot = repo.hot(ref_name)?;
            Hot::concepts(hot.conn())?
        }
    };
    repo.hot(ref_name)?;
    let hot = repo.hots.remove(ref_name).expect("hot");
    let mut findings = Vec::new();
    for c in &targets {
        match checkout::concept_view(repo, hot.conn(), c, None, None) {
            Ok(v) => findings.extend(repo.profile.validate(&v)),
            Err(e) => {
                repo.hots.insert(ref_name.to_string(), hot);
                return Err(e);
            }
        }
    }
    repo.hots.insert(ref_name.to_string(), hot);
    let failing = findings.iter().any(|f| {
        f.level == wisp_profile::Level::Must || (strict && f.level == wisp_profile::Level::Should)
    });
    let v =
        json!({ "profile": repo.profile.id(), "concepts": targets.len(), "findings": findings });
    if failing {
        Err(WispError::validation("validation failed", v))
    } else {
        Ok(v)
    }
}

/// `wisp migrate --to 0.2` (§12): one changeset by `process:wisp-migrate`, then the manifest profile.
pub fn migrate(repo: &mut Repository, ref_name: &str, to: &str, dry_run: bool) -> Result<Value> {
    if repo.profile.okf_version() == to {
        return Ok(json!({ "already": to }));
    }
    if !(repo.profile.okf_version() == "0.1" && to == "0.2") {
        return Err(WispError::not_implemented(&format!(
            "migration {} -> {}",
            repo.profile.okf_version(),
            to
        )));
    }
    repo.hot(ref_name)?;
    let hot = repo.hots.remove(ref_name).expect("hot");
    let mut views = Vec::new();
    for c in Hot::concepts(hot.conn())? {
        match checkout::concept_view(repo, hot.conn(), &c, None, None) {
            Ok(mut v) => {
                // The 0.1 view keeps `timestamp` as a user entry for the planner.
                if let Ok(Some(e)) = Hot::frontmatter_entry(hot.conn(), c.concept, "timestamp") {
                    v.entries.push(("timestamp".into(), e.raw));
                }
                views.push((c, v));
            }
            Err(e) => {
                repo.hots.insert(ref_name.to_string(), hot);
                return Err(e);
            }
        }
    }
    let steps = wisp_profile::plan_migration_01_to_02(
        &views.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(),
    );
    let mut ops = Vec::new();
    for s in &steps {
        match s {
            wisp_profile::MigrationStep::UnsetTimestamp { path, timestamp } => {
                let Some((c, _)) = views.iter().find(|(c, _)| &c.path == path) else {
                    continue;
                };
                let entry = Hot::frontmatter_entry(hot.conn(), c.concept, "timestamp")?;
                let key = repo.scope_key(c.scope)?;
                let key_commit = wisp_crypto::commitment(
                    &key.commit_key(repo.repo_id()),
                    PayloadKind::FrontmatterEntry,
                    b"timestamp",
                );
                let mut op = PlannedOp::new(
                    Op::UnsetKey,
                    Target::FrontmatterKey {
                        concept: c.concept,
                        key_commit,
                    },
                    c.scope,
                )
                .with_payload(
                    PayloadKind::FrontmatterEntry,
                    encode_frontmatter("timestamp", ""),
                )
                .with_precondition(Precondition::EntryVersion {
                    concept: c.concept,
                    key: "timestamp".into(),
                    version: entry.map(|e| e.version),
                })
                .with_path(format!("{path}.md"));
                op.origin = c.gen_actor.map(|a| Origin {
                    actor_id: a,
                    at: timestamp.clone(),
                });
                ops.push(op);
            }
            wisp_profile::MigrationStep::CitationsToSources {
                path,
                heading,
                items,
            } => {
                let Some((c, _)) = views.iter().find(|(c, _)| &c.path == path) else {
                    continue;
                };
                let existing = Hot::sources_of(hot.conn(), c.concept)?.len();
                for (i, item) in items.iter().enumerate() {
                    let id = format!("src-{}", existing + i + 1);
                    ops.push(
                        PlannedOp::new(
                            Op::SourcePut,
                            Target::Source {
                                concept: c.concept,
                                source: id.clone(),
                            },
                            c.scope,
                        )
                        .with_payload(
                            PayloadKind::SourceRecord,
                            encode_source(&id, &json!({ "id": id, "title": item })),
                        )
                        .with_path(format!("{path}.md")),
                    );
                }
                if let Some(sec) = Hot::sections_of(hot.conn(), c.concept)?
                    .into_iter()
                    .find(|s| s.heading.eq_ignore_ascii_case(heading))
                {
                    ops.push(
                        PlannedOp::new(
                            Op::RemoveSection,
                            Target::Section {
                                concept: c.concept,
                                section: sec.section,
                            },
                            sec.scope,
                        )
                        .with_precondition(Precondition::SectionVersion {
                            section: sec.section,
                            version: sec.version,
                        })
                        .with_path(format!("{path}.md")),
                    );
                }
            }
        }
    }
    repo.hots.insert(ref_name.to_string(), hot);
    if dry_run {
        return Ok(json!({ "dry_run": true, "steps": steps.len(), "ops": ops.len() }));
    }
    let mut changeset = None;
    if !ops.is_empty() {
        let mut req = CommitRequest::new(ref_name, "process:wisp-migrate");
        req.message = Some("Migrate okf-0.1 to okf-0.2".into());
        req.skip_policy = true;
        req.ops = ops;
        changeset = Some(commit(repo, req)?.changeset.to_string());
    }
    repo.manifest.repo.profile = "okf-0.2".into();
    repo.manifest.save(&repo.wisp_dir.join("manifest.toml"))?;
    repo.profile = wisp_profile::by_id("okf-0.2").expect("profile");
    if ref_name == MAIN_REF {
        let root = repo.root.clone();
        let idx = checkout::write_tree(repo, ref_name, &root, false, false, true)?;
        idx.save(&checkout::checkout_idx_path(repo, &root))?;
    }
    Ok(json!({ "migrated_to": "0.2", "changeset": changeset, "steps": steps.len() }))
}

// ---- backup / gc / feed / actors / codecs / config ----

pub fn backup(repo: &Repository, dest: &Path, with_keys: Option<&Path>) -> Result<Value> {
    std::fs::create_dir_all(dest)?;
    std::fs::copy(
        repo.wisp_dir.join("manifest.toml"),
        dest.join("manifest.toml"),
    )?;
    {
        let mut dst = rusqlite::Connection::open(dest.join("ledger.db"))
            .map_err(|e| WispError::internal(e.to_string()))?;
        let bk = rusqlite::backup::Backup::new(repo.ledger.conn(), &mut dst)
            .map_err(|e| WispError::internal(e.to_string()))?;
        bk.run_to_completion(100, std::time::Duration::from_millis(5), None)
            .map_err(|e| WispError::internal(e.to_string()))?;
    }
    let src = repo.ledger.payloads_dir();
    let mut n = 0usize;
    for e in walkdir::WalkDir::new(&src).into_iter().flatten() {
        if e.file_type().is_file() {
            let rel = e.path().strip_prefix(&src).unwrap();
            let to = dest.join("payloads").join(rel);
            std::fs::create_dir_all(to.parent().unwrap())?;
            std::fs::copy(e.path(), to)?;
            n += 1;
        }
    }
    let mut v =
        json!({ "dest": dest.display().to_string(), "payloads": n, "includes_keys": false });
    if let Some(kd) = with_keys {
        std::fs::create_dir_all(kd)?;
        let mut dst = rusqlite::Connection::open(kd.join("keystore.db"))
            .map_err(|e| WispError::internal(e.to_string()))?;
        let ks_src = rusqlite::Connection::open(repo.keystore.path())
            .map_err(|e| WispError::internal(e.to_string()))?;
        let bk = rusqlite::backup::Backup::new(&ks_src, &mut dst)
            .map_err(|e| WispError::internal(e.to_string()))?;
        bk.run_to_completion(100, std::time::Duration::from_millis(5), None)
            .map_err(|e| WispError::internal(e.to_string()))?;
        let mk = repo.wisp_dir.join("keys/master.key");
        if mk.exists() {
            std::fs::copy(&mk, kd.join("master.key"))?;
        }
        v["includes_keys"] = json!(true);
        v["keys_dest"] = json!(kd.display().to_string());
    }
    Ok(v)
}

pub fn restore_backup(src: &Path, keys: Option<&Path>, into: &Path) -> Result<Value> {
    let wisp = into.join(".wisp");
    if wisp.exists() {
        return Err(WispError::usage("destination already has .wisp/"));
    }
    std::fs::create_dir_all(wisp.join("keys"))?;
    std::fs::create_dir_all(wisp.join("cache"))?;
    std::fs::copy(src.join("manifest.toml"), wisp.join("manifest.toml"))?;
    std::fs::copy(src.join("ledger.db"), wisp.join("ledger.db"))?;
    let pl = src.join("payloads");
    for e in walkdir::WalkDir::new(&pl).into_iter().flatten() {
        if e.file_type().is_file() {
            let rel = e.path().strip_prefix(&pl).unwrap();
            let to = wisp.join("payloads").join(rel);
            std::fs::create_dir_all(to.parent().unwrap())?;
            std::fs::copy(e.path(), to)?;
        }
    }
    if let Some(kd) = keys {
        std::fs::copy(kd.join("keystore.db"), wisp.join("keys/keystore.db"))?;
        if kd.join("master.key").exists() {
            std::fs::copy(kd.join("master.key"), wisp.join("keys/master.key"))?;
        }
    }
    Ok(json!({ "restored_into": into.display().to_string() }))
}

pub fn gc(repo: &mut Repository) -> Result<Value> {
    // Orphaned keystore entries: keys whose scope never appears in the ledger.
    let scopes: std::collections::HashSet<ScopeId> = Ledger::all_scopes(repo.ledger.conn())?
        .into_iter()
        .map(|s| s.id)
        .collect();
    let mut orphan_keys = 0;
    for k in repo.keystore.list()? {
        if !scopes.contains(&k) && k != repo.repo_scope() {
            repo.keystore.delete(k)?;
            orphan_keys += 1;
        }
    }
    // Hot tiers of refs that no longer exist.
    let refs: std::collections::HashSet<String> = Ledger::refs(repo.ledger.conn())?
        .into_iter()
        .map(|(n, _)| wisp_store::hot::ref_id(&n))
        .collect();
    let mut stale_hot = 0;
    if let Ok(rd) = std::fs::read_dir(repo.wisp_dir.join("cache/refs")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !refs.contains(&name) {
                let _ = std::fs::remove_dir_all(e.path());
                stale_hot += 1;
            }
        }
    }
    repo.keystore.checkpoint()?;
    repo.ledger.checkpoint()?;
    Ok(json!({ "orphan_keys_removed": orphan_keys, "stale_hot_tiers_removed": stale_hot }))
}

/// `wisp feed --since <seq>`: events never contain content (§14.1).
pub fn feed(repo: &Repository, since: i64) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (seq, c) in Ledger::changes_since(repo.ledger.conn(), since)? {
        let event = match c.op {
            Op::RemoveSection | Op::Retract => "remove",
            Op::Restore => "restore",
            Op::Erase => "erase",
            Op::RegisterActor | Op::ActorKey | Op::GroupAdd | Op::GroupRemove => continue,
            _ => "upsert",
        };
        let mut v = json!({
            "seq": seq, "changeset": c.changeset.to_string(), "change": c.id.to_string(), "op": c.op.as_str(),
            "concept": c.target.concept().map(|x| x.to_string()), "section": c.target.section().map(|x| x.to_string()),
            "asset": match &c.target { Target::Asset { asset } => Some(asset.to_string()), _ => None },
            "scope": c.scope.to_string(), "version": c.id.to_string(), "event": event
        });
        if let Target::Erase { scopes } = &c.target {
            v["scopes"] = json!(scopes.iter().map(|s| s.to_string()).collect::<Vec<_>>());
            // List affected concepts/sections from scope rows.
            let mut affected = Vec::new();
            for s in scopes {
                if let Some(row) = Ledger::scope(repo.ledger.conn(), *s)? {
                    match row.kind {
                        ScopeKind::Concept { concept } => affected.push(json!({ "concept": concept.to_string() })),
                        ScopeKind::Section { concept, section } => affected.push(json!({ "concept": concept.to_string(), "section": section.to_string() })),
                        ScopeKind::Asset { asset } => affected.push(json!({ "asset": asset.to_string() })),
                        _ => {}
                    }
                }
            }
            v["affected"] = json!(affected);
        }
        out.push(v);
    }
    Ok(out)
}

/// `wisp index`: run built-in subscribers to the head (tokens/links/views are maintained inline in this build).
pub fn index(repo: &mut Repository, rebuild: Option<&str>) -> Result<Value> {
    let max = Ledger::max_seq(repo.ledger.conn())?;
    let mut ran = Vec::new();
    for s in repo.manifest.subscribers.clone() {
        if rebuild.map(|r| r == s.name).unwrap_or(false) {
            Ledger::set_cursor(repo.ledger.conn(), &s.name, 0)?;
        }
        if s.builtin.is_some() {
            Ledger::set_cursor(repo.ledger.conn(), &s.name, max)?;
            ran.push(json!({ "subscriber": s.name, "cursor": max, "builtin": s.builtin }));
        } else {
            ran.push(json!({ "subscriber": s.name, "cursor": Ledger::cursor(repo.ledger.conn(), &s.name)?, "external": true, "note": "external subscribers are driven by `wisp feed`" }));
        }
    }
    Ok(json!({ "head_seq": max, "subscribers": ran }))
}

pub fn actors(repo: &Repository) -> Result<Value> {
    let mut out = Vec::new();
    for a in Ledger::actors(repo.ledger.conn())? {
        let name = repo.actor_name(a.id);
        out.push(json!({ "id": a.id.to_string(), "kind": a.kind.as_str(), "name": name.clone().unwrap_or_else(|| "erased".into()), "erased": name.is_none(), "has_key": a.public_key.is_some() }));
    }
    Ok(json!(out))
}

/// Generate an Ed25519 keypair, store the seed locally, and register the public key on the actor.
pub fn actor_gen_key(repo: &mut Repository, ref_name: &str, actor: &str) -> Result<Value> {
    let row = repo.find_actor(actor)?.ok_or_else(|| {
        WispError::not_found(format!(
            "no actor {actor}; it registers on its first commit"
        ))
    })?;
    if row.public_key.is_some() {
        return Err(WispError::usage(
            "actor already has a public key; use `wisp actor key` to rotate from a file",
        ));
    }
    let sk = wisp_crypto::sig::generate();
    let seed = wisp_crypto::sig::seed_bytes(&sk);
    let pk = wisp_crypto::sig::verify_key_bytes(&sk);
    crate::signing::save_signing_seed(&repo.wisp_dir, row.id, &seed)?;
    let out = actor_key(repo, ref_name, actor, pk.as_slice())?;
    let seed_path = crate::signing::seed_path(&repo.wisp_dir, row.id);
    Ok(json!({
        "changeset": out["changeset"],
        "actor": out["actor"],
        "public_key": hex::encode(pk),
        "seed": seed_path.display().to_string(),
    }))
}

pub fn actor_key(
    repo: &mut Repository,
    ref_name: &str,
    actor: &str,
    key_bytes: &[u8],
) -> Result<Value> {
    let row = repo.find_actor(actor)?.ok_or_else(|| {
        WispError::not_found(format!(
            "no actor {actor}; it registers on its first commit"
        ))
    })?;
    let op = PlannedOp::new(Op::ActorKey, Target::Actor { actor: row.id }, row.scope)
        .with_payload(PayloadKind::ActorKey, key_bytes.to_vec());
    let mut req = CommitRequest::new(ref_name, actor);
    req.ops = vec![op];
    req.skip_policy = true;
    let out = commit(repo, req)?;
    Ok(json!({ "changeset": out.changeset.to_string(), "actor": row.id.to_string() }))
}

pub fn codec(repo: &mut Repository, action: &str, id: Option<&str>) -> Result<Value> {
    let m = &mut repo.manifest.codecs;
    match (action, id) {
        ("list", _) => {}
        ("add", Some(id)) => {
            if id.starts_with("emb:") {
                if !m.embedding.iter().any(|x| x == id) {
                    m.embedding.push(id.to_string());
                }
            } else if id.starts_with("tok:") {
                if !m.tokenizer.iter().any(|x| x == id) {
                    m.tokenizer.push(id.to_string());
                }
            } else {
                return Err(WispError::usage("codec IDs start with emb: or tok:"));
            }
        }
        ("default", Some(id)) => {
            if id.starts_with("emb:") {
                m.default_embedding = Some(id.to_string());
            } else {
                m.default_tokenizer = Some(id.to_string());
            }
        }
        ("remove", Some(id)) => {
            m.embedding.retain(|x| x != id);
            m.tokenizer.retain(|x| x != id);
            if m.default_embedding.as_deref() == Some(id) {
                m.default_embedding = None;
            }
        }
        _ => return Err(WispError::usage("codec add|default|list|remove <id>")),
    }
    let v = serde_json::to_value(&repo.manifest.codecs)?;
    repo.manifest.save(&repo.wisp_dir.join("manifest.toml"))?;
    Ok(v)
}

pub fn config(repo: &mut Repository, key: &str, value: Option<&str>) -> Result<Value> {
    match (key, value) {
        ("actor", None) => Ok(json!({ "actor": repo.manifest.config.actor })),
        ("actor", Some(v)) => {
            ActorKind::classify(v).ok_or_else(|| {
                WispError::usage("actor must be <producer>/<version>, human:<id>, or process:<id>")
            })?;
            repo.manifest.config.actor = Some(v.to_string());
            repo.manifest.save(&repo.wisp_dir.join("manifest.toml"))?;
            Ok(json!({ "actor": v }))
        }
        ("render.erasure_markers", None) => {
            Ok(json!({ "render.erasure_markers": repo.manifest.render.erasure_markers }))
        }
        ("render.erasure_markers", Some(v)) => {
            repo.manifest.render.erasure_markers = v == "true";
            repo.manifest.save(&repo.wisp_dir.join("manifest.toml"))?;
            Ok(json!({ "render.erasure_markers": repo.manifest.render.erasure_markers }))
        }
        ("commit.busy_timeout_ms", Some(v)) => {
            repo.manifest.commit.busy_timeout_ms = v
                .parse()
                .map_err(|_| WispError::usage("integer expected"))?;
            repo.manifest.save(&repo.wisp_dir.join("manifest.toml"))?;
            Ok(json!({ "commit.busy_timeout_ms": repo.manifest.commit.busy_timeout_ms }))
        }
        ("commit.retries", Some(v)) => {
            repo.manifest.commit.retries = v
                .parse()
                .map_err(|_| WispError::usage("integer expected"))?;
            repo.manifest.save(&repo.wisp_dir.join("manifest.toml"))?;
            Ok(json!({ "commit.retries": repo.manifest.commit.retries }))
        }
        (k, None) => Err(WispError::not_found(format!("unknown config key {k}"))),
        (k, Some(_)) => Err(WispError::usage(format!("config key {k} is not settable"))),
    }
}
