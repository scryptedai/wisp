//! The record pipeline (§9): working-tree edits → one changeset.

use crate::checkout::{self, CheckoutIndex};
use crate::commit::{CommitOutcome, CommitRequest, PlannedOp, PolicyFacts, Precondition};
use crate::merge;
use crate::payloads::*;
use crate::repo::{Repository, MAIN_REF};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use wisp_match::{
    match_concepts, match_sections, ConceptDecision, Context, Decision, PriorSection,
};
use wisp_model::*;
use wisp_parse::{Anchor, ParsedFile};
use wisp_profile::{ConceptView, Level};
use wisp_store::{Hot, Ledger};

#[derive(Clone, Debug, Default)]
pub struct RecordOptions {
    pub ref_name: Option<String>,
    pub actor: Option<String>,
    pub message: Option<String>,
    pub reason: Option<Reason>,
    pub note: Option<String>,
    pub inputs: Vec<InputRef>,
    pub origin: Option<Origin>,
    pub dry_run: bool,
    pub explain: bool,
    pub full: bool,
    pub allow_nonconformant: bool,
    pub adopt_generated: bool,
    pub keep_format: bool,
    /// After a successful record, clear `.wisp/merge.json` when the recorded ref matches `into`.
    pub resolve_merge: bool,
}

/// What `status` reports (§16).
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Status {
    pub ref_name: String,
    pub head: Option<String>,
    pub changed: Vec<String>,
    pub new: Vec<String>,
    pub missing: Vec<String>,
    pub nonconformant: Vec<String>,
    pub generated_modified: Vec<String>,
    pub warnings: Vec<String>,
    pub pending_erasures: usize,
}

/// The planned changeset, before commit.
pub struct Plan {
    pub ops: Vec<PlannedOp>,
    pub status: Status,
    pub explanations: Vec<Value>,
    pub validation: Vec<wisp_profile::Finding>,
    pub base: Option<ChangesetId>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct RecordResult {
    pub changeset: Option<String>,
    pub seq: Option<i64>,
    pub ops: usize,
    pub status: Status,
    pub explanations: Vec<Value>,
    pub warnings: Vec<Value>,
}

struct Scanned {
    rel: String,
    abs: PathBuf,
    changed: bool,
}

fn load_ignore(root: &Path) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    b.add(Glob::new(".wisp/**").unwrap());
    b.add(Glob::new(".wisp").unwrap());
    b.add(Glob::new("**/*.wisp-tmp").unwrap());
    b.add(Glob::new(".wispignore").unwrap());
    if let Ok(s) = std::fs::read_to_string(root.join(".wispignore")) {
        for line in s.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            let l = l.trim_start_matches('/');
            if let Some(dir) = l.strip_suffix('/') {
                let _ = Glob::new(dir).map(|g| b.add(g));
                let _ = Glob::new(&format!("{dir}/**")).map(|g| b.add(g));
                let _ = Glob::new(&format!("**/{dir}/**")).map(|g| b.add(g));
            } else {
                let _ = Glob::new(l).map(|g| b.add(g));
                let _ = Glob::new(&format!("**/{l}")).map(|g| b.add(g));
            }
        }
    }
    b.build().unwrap_or_else(|_| GlobSet::empty())
}

/// Walk the tree and classify files against `checkout.idx` (§9 step 2).
fn scan(
    repo: &Repository,
    tree_root: &Path,
    idx: &CheckoutIndex,
    full: bool,
) -> Result<(Vec<Scanned>, Vec<String>, Vec<String>)> {
    let ignore = load_ignore(&repo.root);
    let mut out = Vec::new();
    let mut generated_modified = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for entry in walkdir::WalkDir::new(tree_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let rel = e.path().strip_prefix(tree_root).unwrap_or(e.path());
            let rel_s = normalize_path(&rel.to_string_lossy());
            rel_s.is_empty() || !ignore.is_match(&rel_s)
        })
    {
        let entry = entry.map_err(|e| WispError::internal(format!("walk: {e}")))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = normalize_path(
            &entry
                .path()
                .strip_prefix(tree_root)
                .unwrap_or(entry.path())
                .to_string_lossy(),
        );
        if rel.is_empty() || ignore.is_match(&rel) {
            continue;
        }
        if is_reserved_file(&rel) {
            if let Some(rec) = idx.files.get(&rel) {
                if checkout::file_changed(entry.path(), rec) {
                    generated_modified.push(rel.clone());
                }
            }
            continue;
        }
        seen.insert(rel.clone());
        let changed = match idx.files.get(&rel) {
            Some(rec) if !full => checkout::file_changed(entry.path(), rec),
            _ => true,
        };
        out.push(Scanned {
            rel,
            abs: entry.path().to_path_buf(),
            changed,
        });
    }
    let missing: Vec<String> = idx
        .files
        .iter()
        .filter(|(rel, rec)| rec.kind != "generated" && !seen.contains(*rel))
        .map(|(rel, _)| rel.clone())
        .collect();
    Ok((out, generated_modified, missing))
}

struct HotCtx<'a> {
    conn: &'a rusqlite::Connection,
    ledger: &'a rusqlite::Connection,
    /// Anchors present in the files being recorded, per owning concept.
    anchors_in_files: &'a HashMap<ConceptId, HashSet<SectionId>>,
}

impl<'a> Context for HotCtx<'a> {
    fn owner_of(&self, section: SectionId) -> Option<ConceptId> {
        Hot::section(self.conn, section)
            .ok()
            .flatten()
            .map(|s| s.concept)
    }
    fn owner_still_has(&self, section: SectionId) -> bool {
        match self.owner_of(section) {
            Some(owner) => match self.anchors_in_files.get(&owner) {
                Some(set) => set.contains(&section),
                // The owner's file is not being recorded: it still has the section.
                None => true,
            },
            None => false,
        }
    }
    fn is_erased(&self, section: SectionId) -> bool {
        Ledger::scope_of_section(self.ledger, section)
            .ok()
            .flatten()
            .map(|s| s.erased)
            .unwrap_or(false)
    }
}

fn section_body_of(s: &wisp_parse::ParsedSection, after: Option<SectionId>) -> SectionBody {
    SectionBody {
        level: s.level,
        heading: s.heading.clone(),
        heading_line: s.heading_line.clone(),
        body: s.body.clone(),
        after,
        at_start: false,
    }
}

/// Build the plan for the working tree (§9 steps 1–8). Shared by `record`, `status`, and `diff`.
pub fn plan(repo: &mut Repository, opts: &RecordOptions) -> Result<Plan> {
    let ref_name = opts
        .ref_name
        .clone()
        .unwrap_or_else(|| MAIN_REF.to_string());
    let idx_path = checkout::checkout_idx_path(repo, &repo.root.clone());
    let idx = CheckoutIndex::load(&idx_path)?.unwrap_or_default();
    let (scanned, generated_modified, missing) = scan(repo, &repo.root, &idx, opts.full)?;
    let managed: Vec<&str> = repo.profile.managed_keys().to_vec();
    let identity_key = repo.identity_key().to_string();
    let head = repo.head(&ref_name)?;

    let mut status = Status {
        ref_name: ref_name.clone(),
        head: head.map(|h| h.to_string()),
        ..Default::default()
    };
    status.generated_modified = generated_modified.clone();
    status.missing = missing.clone();
    status.pending_erasures = Ledger::pending_erasures(repo.ledger.conn())?.len();

    // Parse changed files.
    struct Parsed {
        rel: String,
        text: String,
        file: ParsedFile,
        concept_path: String,
    }
    let mut concepts_in: Vec<Parsed> = Vec::new();
    let mut assets_in: Vec<(String, Vec<u8>)> = Vec::new();
    for s in scanned.iter().filter(|s| s.changed) {
        if s.rel.ends_with(".md") {
            let bytes = std::fs::read(&s.abs)?;
            let text = String::from_utf8(bytes).map_err(|_| {
                WispError::validation(
                    format!("{}: not valid UTF-8", s.rel),
                    json!([{ "path": s.rel, "code": "utf8" }]),
                )
            })?;
            let file = wisp_parse::parse_file(&text);
            if file.frontmatter.is_none()
                || file.frontmatter.as_ref().map(|f| f.opaque).unwrap_or(false)
            {
                status.nonconformant.push(s.rel.clone());
                if !opts.allow_nonconformant {
                    continue;
                }
            }
            for w in &file.sectioned.warnings {
                status.warnings.push(format!("{}: {}", s.rel, w));
            }
            concepts_in.push(Parsed {
                rel: s.rel.clone(),
                concept_path: concept_path_of(&s.rel),
                text,
                file,
            });
        } else {
            assets_in.push((s.rel.clone(), std::fs::read(&s.abs)?));
        }
    }

    repo.hot(&ref_name)?;
    let hot = repo.hots.remove(&ref_name).expect("hot");
    let result = (|| -> Result<Plan> {
        let conn = hot.conn();
        let known: HashMap<ConceptId, String> = Hot::concepts(conn)?
            .into_iter()
            .map(|c| (c.concept, c.path))
            .collect();
        let path_to_known: HashMap<String, ConceptId> =
            known.iter().map(|(k, v)| (v.clone(), *k)).collect();

        // Concept identity (§7.4), with the pragmatic repair of a stripped wisp_id (DECISIONS.md).
        let files: Vec<(String, Option<ConceptId>)> = concepts_in
            .iter()
            .map(|p| {
                let id = p
                    .file
                    .wisp_id(&identity_key)
                    .or_else(|| path_to_known.get(&p.concept_path).copied());
                (p.concept_path.clone(), id)
            })
            .collect();
        let decisions = match_concepts(&files, &known);

        // Anchors present in the recorded files, per concept (for rule 7.5.2).
        let mut anchors_in_files: HashMap<ConceptId, HashSet<SectionId>> = HashMap::new();
        for (p, d) in concepts_in.iter().zip(decisions.iter()) {
            let cid = match d {
                ConceptDecision::Known { id, .. } => *id,
                ConceptDecision::New { id } => *id,
                ConceptDecision::Copy { new_id, .. } => *new_id,
            };
            let set = anchors_in_files.entry(cid).or_default();
            for s in p.file.sections() {
                if let Some(Anchor::Valid(a)) = &s.anchor {
                    set.insert(*a);
                }
            }
        }
        let ctx = HotCtx {
            conn,
            ledger: repo.ledger.conn(),
            anchors_in_files: &anchors_in_files,
        };

        let mut ops: Vec<PlannedOp> = Vec::new();
        let mut explanations: Vec<Value> = Vec::new();
        let mut views: Vec<ConceptView> = Vec::new();

        for (p, d) in concepts_in.iter().zip(decisions.iter()) {
            let (cid, is_new, moved_from, copy_of) = match d {
                ConceptDecision::Known { id, moved_from } => (*id, false, moved_from.clone(), None),
                ConceptDecision::New { id } => (*id, true, None, None),
                ConceptDecision::Copy { of, new_id } => (*new_id, true, None, Some(*of)),
            };
            let concept_type = p.file.type_value();
            let fm = p.file.frontmatter.clone().unwrap_or_default();
            let user_entries: Vec<(String, String)> = fold_entries(&fm)
                .into_iter()
                .filter(|(k, _)| k != &identity_key && !managed.contains(&k.as_str()))
                .collect();
            let sections = p.file.sections();
            let base = idx.concepts.get(&cid.to_string());

            if is_new {
                if idx
                    .files
                    .get(&p.rel)
                    .map(|r| r.kind == "concept")
                    .unwrap_or(false)
                {
                    status.changed.push(p.rel.clone());
                } else {
                    status.new.push(p.rel.clone());
                }
                let cscope = ScopeId::new();
                // Keys for new scopes are written before the ledger commit (§10.2 step 1); an orphan is harmless.
                repo.new_scope_key(cscope)?;
                let mut op =
                    PlannedOp::new(Op::CreateConcept, Target::Concept { concept: cid }, cscope)
                        .with_new_scope(ScopeKind::Concept { concept: cid })
                        .with_payload(PayloadKind::Path, p.concept_path.as_bytes().to_vec())
                        .with_precondition(Precondition::PathFree(p.concept_path.clone()))
                        .with_path(&p.rel);
                op.origin = opts.origin.clone();
                op.policy = PolicyFacts {
                    concept_type: concept_type.clone(),
                    ..Default::default()
                };
                ops.push(op);
                if let Some(of) = copy_of {
                    explanations.push(json!({ "path": p.rel, "rule": "7.4.2 wisp_id seen in two files: this one is a copy", "copy_of": of.to_string(), "new_id": cid.to_string() }));
                } else if p.file.wisp_id(&identity_key).is_none()
                    && !path_to_known.contains_key(&p.concept_path)
                {
                    explanations.push(json!({ "path": p.rel, "rule": "7.4.3 no wisp_id: new concept", "id": cid.to_string() }));
                }
                for (k, raw) in &user_entries {
                    let key_commit = key_commit(repo, cscope, k);
                    let mut op = PlannedOp::new(
                        Op::SetKey,
                        Target::FrontmatterKey {
                            concept: cid,
                            key_commit,
                        },
                        cscope,
                    )
                    .with_payload(PayloadKind::FrontmatterEntry, encode_frontmatter(k, raw))
                    .with_path(&p.rel);
                    op.policy = PolicyFacts {
                        concept_type: concept_type.clone(),
                        key: Some(k.clone()),
                        ..Default::default()
                    };
                    op.origin = opts.origin.clone();
                    ops.push(op);
                }
                // Managed `sources` written by hand become source_put operations.
                for (sid, rec) in parsed_sources(&fm) {
                    let mut op = PlannedOp::new(
                        Op::SourcePut,
                        Target::Source {
                            concept: cid,
                            source: sid.clone(),
                        },
                        cscope,
                    )
                    .with_payload(PayloadKind::SourceRecord, encode_source(&sid, &rec))
                    .with_path(&p.rel);
                    op.policy = PolicyFacts {
                        concept_type: concept_type.clone(),
                        ..Default::default()
                    };
                    ops.push(op);
                }
                if let Some(st) = parsed_status(&fm) {
                    let op =
                        PlannedOp::new(Op::SetStatus, Target::Concept { concept: cid }, cscope)
                            .with_payload(PayloadKind::Status, st.as_str().as_bytes().to_vec())
                            .with_path(&p.rel);
                    ops.push(op);
                }
                let mut prev: Option<SectionId> = None;
                for (i, s) in sections.iter().enumerate() {
                    // Adopt a well-formed unknown anchor (rule 5) unless it is known elsewhere or erased.
                    let sid = match &s.anchor {
                        Some(Anchor::Valid(a))
                            if ctx.owner_of(*a).is_none()
                                && !ctx.is_erased(*a)
                                && !is_known_section(conn, *a) =>
                        {
                            *a
                        }
                        _ => SectionId::new(),
                    };
                    let sscope = ScopeId::new();
                    let mut op = PlannedOp::new(
                        Op::AddSection,
                        Target::Section {
                            concept: cid,
                            section: sid,
                        },
                        sscope,
                    )
                    .with_new_scope(ScopeKind::Section {
                        concept: cid,
                        section: sid,
                    })
                    .with_payload(PayloadKind::Section, section_body_of(s, prev).encode())
                    .with_path(&p.rel);
                    op.policy = PolicyFacts {
                        concept_type: concept_type.clone(),
                        section_heading: Some(s.heading.clone()),
                        ..Default::default()
                    };
                    op.origin = opts.origin.clone();
                    if copy_of.is_some() {
                        if let Some(Anchor::Valid(a)) = &s.anchor {
                            op.lineage = Some(Lineage::CopiedFrom {
                                concept: copy_of.unwrap(),
                                section: *a,
                            });
                        }
                    }
                    ops.push(op);
                    if opts.explain {
                        explanations.push(json!({ "path": p.rel, "section": i, "heading": s.heading, "rule": "7.5.6 new section", "id": sid.to_string() }));
                    }
                    if s.level != 0 {
                        prev = Some(sid);
                    }
                }
            } else {
                status.changed.push(p.rel.clone());
                let cstate = Hot::concept(conn, cid)?
                    .ok_or_else(|| WispError::internal("known concept missing from hot tier"))?;
                let cscope = cstate.scope;
                let type_now = concept_type.clone().or(cstate.type_.clone());
                // Move.
                if let Some(from) = &moved_from {
                    let mut op = PlannedOp::new(Op::Move, Target::Concept { concept: cid }, cscope)
                        .with_payload(PayloadKind::Path, p.concept_path.as_bytes().to_vec())
                        .with_precondition(Precondition::PathFree(p.concept_path.clone()))
                        .with_path(&p.rel);
                    op.policy = PolicyFacts {
                        concept_type: type_now.clone(),
                        ..Default::default()
                    };
                    ops.push(op);
                    explanations.push(json!({ "path": p.rel, "rule": "7.4.1 known wisp_id at a new path: move", "from": from }));
                }
                // Frontmatter entries, compared against the base the author read (§9 step 1).
                let hot_entries = base_entries(repo, conn, cid, base)?;
                let mut seen_keys: HashSet<String> = HashSet::new();
                for (k, raw) in &user_entries {
                    seen_keys.insert(k.clone());
                    let existing = hot_entries.iter().find(|e| &e.key == k);
                    let changed = match existing {
                        None => true,
                        Some(e) => {
                            let a = wisp_parse::frontmatter::entry_value(&e.raw);
                            let b = wisp_parse::frontmatter::entry_value(raw);
                            if opts.keep_format {
                                e.raw != *raw
                            } else {
                                a != b || (a.is_none() && e.raw != *raw)
                            }
                        }
                    };
                    if changed {
                        let base_v = base
                            .and_then(|b| b.entries.get(k).copied())
                            .or(existing.map(|e| e.version));
                        let key_commit = key_commit(repo, cscope, k);
                        let mut op = PlannedOp::new(
                            Op::SetKey,
                            Target::FrontmatterKey {
                                concept: cid,
                                key_commit,
                            },
                            cscope,
                        )
                        .with_payload(PayloadKind::FrontmatterEntry, encode_frontmatter(k, raw))
                        .with_precondition(Precondition::EntryVersion {
                            concept: cid,
                            key: k.clone(),
                            version: base_v,
                        })
                        .with_path(&p.rel);
                        op.policy = PolicyFacts {
                            concept_type: type_now.clone(),
                            key: Some(k.clone()),
                            ..Default::default()
                        };
                        op.origin = opts.origin.clone();
                        ops.push(op);
                    }
                }
                for e in hot_entries
                    .iter()
                    .filter(|e| !seen_keys.contains(&e.key) && !managed.contains(&e.key.as_str()))
                {
                    let base_v = base
                        .and_then(|b| b.entries.get(&e.key).copied())
                        .unwrap_or(e.version);
                    let key_commit = key_commit(repo, cscope, &e.key);
                    let mut op = PlannedOp::new(
                        Op::UnsetKey,
                        Target::FrontmatterKey {
                            concept: cid,
                            key_commit,
                        },
                        cscope,
                    )
                    .with_payload(
                        PayloadKind::FrontmatterEntry,
                        encode_frontmatter(&e.key, ""),
                    )
                    .with_precondition(Precondition::EntryVersion {
                        concept: cid,
                        key: e.key.clone(),
                        version: Some(base_v),
                    })
                    .with_path(&p.rel);
                    op.policy = PolicyFacts {
                        concept_type: type_now.clone(),
                        key: Some(e.key.clone()),
                        ..Default::default()
                    };
                    ops.push(op);
                }
                // Managed keys edited by hand: sources and status map to operations; others are ignored.
                let hot_sources = Hot::sources_of(conn, cid)?;
                let parsed_src = parsed_sources(&fm);
                if fm.get("sources").is_some() {
                    for (sid, rec) in &parsed_src {
                        let existing = hot_sources.iter().find(|s| &s.source_id == sid);
                        if existing.map(|e| &e.record != rec).unwrap_or(true) {
                            let base_v = base
                                .and_then(|b| b.sources.get(sid).copied())
                                .or(existing.map(|e| e.version));
                            ops.push(
                                PlannedOp::new(
                                    Op::SourcePut,
                                    Target::Source {
                                        concept: cid,
                                        source: sid.clone(),
                                    },
                                    cscope,
                                )
                                .with_payload(PayloadKind::SourceRecord, encode_source(sid, rec))
                                .with_precondition(Precondition::SourceVersion {
                                    concept: cid,
                                    id: sid.clone(),
                                    version: base_v,
                                })
                                .with_path(&p.rel),
                            );
                        }
                    }
                    for e in hot_sources
                        .iter()
                        .filter(|e| !parsed_src.iter().any(|(s, _)| s == &e.source_id))
                    {
                        ops.push(
                            PlannedOp::new(
                                Op::SourceRemove,
                                Target::Source {
                                    concept: cid,
                                    source: e.source_id.clone(),
                                },
                                cscope,
                            )
                            .with_precondition(Precondition::SourceVersion {
                                concept: cid,
                                id: e.source_id.clone(),
                                version: Some(e.version),
                            })
                            .with_path(&p.rel),
                        );
                    }
                }
                if let Some(st) = parsed_status(&fm) {
                    if cstate.status != Some(st) {
                        ops.push(
                            PlannedOp::new(Op::SetStatus, Target::Concept { concept: cid }, cscope)
                                .with_payload(PayloadKind::Status, st.as_str().as_bytes().to_vec())
                                .with_precondition(Precondition::ConceptLive(cid))
                                .with_path(&p.rel),
                        );
                    }
                }

                // Sections (§7.5), matched against the base the author read.
                let prior_states = base_sections(repo, conn, cid, base)?;
                let prior: Vec<PriorSection> = prior_states
                    .iter()
                    .map(|s| PriorSection {
                        id: s.section,
                        level: s.level,
                        heading: s.heading.clone(),
                        heading_line: s.heading_line.clone(),
                        body: s.body.clone(),
                    })
                    .collect();
                let m = match_sections(sections, &prior, &ctx);
                for r in &m.reports {
                    status.warnings.push(format!("{}: {}", p.rel, r));
                }
                let base_section_v = |sid: SectionId| -> Option<ChangeId> {
                    base.and_then(|b| b.sections.iter().find(|(s, _)| *s == sid).map(|(_, v)| *v))
                        .or_else(|| {
                            prior_states
                                .iter()
                                .find(|s| s.section == sid)
                                .map(|s| s.version)
                        })
                };
                let merged_into: HashMap<SectionId, Vec<SectionId>> = m
                    .removed
                    .iter()
                    .filter_map(|r| r.merged_into.map(|into| (into, r.id)))
                    .fold(HashMap::new(), |mut acc, (into, id)| {
                        acc.entry(into).or_default().push(id);
                        acc
                    });
                let mut prev: Option<SectionId> = None;
                let mut final_order: Vec<SectionId> = Vec::new();
                for (i, (s, d)) in sections.iter().zip(m.decisions.iter()).enumerate() {
                    let sid = d.id();
                    final_order.push(sid);
                    if opts.explain {
                        explanations.push(json!({ "path": p.rel, "section": i, "heading": s.heading, "rule": d.rule(), "id": sid.to_string() }));
                    }
                    let facts = PolicyFacts {
                        concept_type: type_now.clone(),
                        section_heading: Some(s.heading.clone()),
                        ..Default::default()
                    };
                    match d {
                        Decision::Same { id } | Decision::Repaired { id } => {
                            let ps = prior_states
                                .iter()
                                .find(|x| x.section == *id)
                                .expect("prior");
                            let changed = ps.level != s.level
                                || ps.heading_line != s.heading_line
                                || ps.body != s.body;
                            if changed {
                                let mut op = PlannedOp::new(
                                    Op::UpdateSection,
                                    Target::Section {
                                        concept: cid,
                                        section: *id,
                                    },
                                    ps.scope,
                                )
                                .with_payload(
                                    PayloadKind::Section,
                                    section_body_of(s, None).encode(),
                                )
                                .with_precondition(Precondition::SectionVersion {
                                    section: *id,
                                    version: base_section_v(*id).unwrap_or(ps.version),
                                })
                                .with_path(&p.rel);
                                op.policy = facts;
                                op.origin = opts.origin.clone();
                                if let Some(from) = merged_into.get(id) {
                                    op.lineage = Some(Lineage::MergedFrom {
                                        sections: from.clone(),
                                    });
                                }
                                ops.push(op);
                            }
                        }
                        Decision::Moved { id, from } => {
                            let scope = Ledger::scope_of_section(repo.ledger.conn(), *id)?
                                .map(|s| s.id)
                                .unwrap_or_else(ScopeId::new);
                            let cur_v = Hot::section(conn, *id)?.map(|x| x.version);
                            let mut op = PlannedOp::new(
                                Op::MoveSection,
                                Target::Section {
                                    concept: cid,
                                    section: *id,
                                },
                                scope,
                            )
                            .with_payload(PayloadKind::Section, section_body_of(s, prev).encode())
                            .with_path(&p.rel);
                            if let Some(v) = cur_v {
                                op.precondition = Precondition::SectionVersion {
                                    section: *id,
                                    version: v,
                                };
                            }
                            op.lineage = Some(Lineage::MovedFrom { concept: *from });
                            op.policy = facts;
                            ops.push(op);
                        }
                        Decision::Copied {
                            new_id,
                            from_concept,
                            from_section,
                        } => {
                            let sscope = ScopeId::new();
                            let mut op = PlannedOp::new(
                                Op::AddSection,
                                Target::Section {
                                    concept: cid,
                                    section: *new_id,
                                },
                                sscope,
                            )
                            .with_new_scope(ScopeKind::Section {
                                concept: cid,
                                section: *new_id,
                            })
                            .with_payload(PayloadKind::Section, section_body_of(s, prev).encode())
                            .with_precondition(Precondition::ConceptLive(cid))
                            .with_path(&p.rel);
                            op.lineage = Some(Lineage::CopiedFrom {
                                concept: *from_concept,
                                section: *from_section,
                            });
                            op.policy = facts;
                            ops.push(op);
                        }
                        Decision::Reminted { new_id, .. }
                        | Decision::Added { new_id }
                        | Decision::Adopted { id: new_id } => {
                            let sscope = ScopeId::new();
                            let mut op = PlannedOp::new(
                                Op::AddSection,
                                Target::Section {
                                    concept: cid,
                                    section: *new_id,
                                },
                                sscope,
                            )
                            .with_new_scope(ScopeKind::Section {
                                concept: cid,
                                section: *new_id,
                            })
                            .with_payload(PayloadKind::Section, section_body_of(s, prev).encode())
                            .with_precondition(Precondition::ConceptLive(cid))
                            .with_path(&p.rel);
                            op.policy = facts;
                            op.origin = opts.origin.clone();
                            ops.push(op);
                        }
                        Decision::Split { new_id, from } => {
                            let sscope = ScopeId::new();
                            let mut op = PlannedOp::new(
                                Op::Split,
                                Target::Section {
                                    concept: cid,
                                    section: *new_id,
                                },
                                sscope,
                            )
                            .with_new_scope(ScopeKind::Section {
                                concept: cid,
                                section: *new_id,
                            })
                            .with_payload(PayloadKind::Section, section_body_of(s, prev).encode())
                            .with_precondition(Precondition::ConceptLive(cid))
                            .with_path(&p.rel);
                            op.lineage = Some(Lineage::SplitFrom { section: *from });
                            op.policy = facts;
                            ops.push(op);
                        }
                    }
                    if s.level != 0 {
                        prev = Some(sid);
                    }
                }
                for r in &m.removed {
                    let ps = prior_states
                        .iter()
                        .find(|x| x.section == r.id)
                        .expect("prior");
                    let mut op = PlannedOp::new(
                        Op::RemoveSection,
                        Target::Section {
                            concept: cid,
                            section: r.id,
                        },
                        ps.scope,
                    )
                    .with_precondition(Precondition::SectionVersion {
                        section: r.id,
                        version: base_section_v(r.id).unwrap_or(ps.version),
                    })
                    .with_path(&p.rel);
                    op.policy = PolicyFacts {
                        concept_type: type_now.clone(),
                        section_heading: Some(ps.heading.clone()),
                        ..Default::default()
                    };
                    ops.push(op);
                    if opts.explain {
                        explanations.push(json!({ "path": p.rel, "removed": r.id.to_string(), "rule": if r.merged_into.is_some() { "7.5.7 merged into surviving section" } else { "7.5.7 removed" } }));
                    }
                }
                // Order: compare surviving prior sections' relative order with the new order.
                let prior_order: Vec<SectionId> = prior_states.iter().map(|s| s.section).collect();
                let survivors_new: Vec<SectionId> = final_order
                    .iter()
                    .filter(|s| prior_order.contains(s))
                    .copied()
                    .collect();
                let survivors_old: Vec<SectionId> = prior_order
                    .iter()
                    .filter(|s| final_order.contains(s))
                    .copied()
                    .collect();
                if survivors_new != survivors_old {
                    ops.push(
                        PlannedOp::new(
                            Op::ReorderSections,
                            Target::Concept { concept: cid },
                            cscope,
                        )
                        .with_payload(PayloadKind::SectionOrder, encode_order(&final_order))
                        .with_precondition(Precondition::SectionOrder {
                            concept: cid,
                            order: prior_order.clone(),
                        })
                        .with_path(&p.rel),
                    );
                }
            }

            // Validation view for this concept.
            views.push(ConceptView {
                path: p.concept_path.clone(),
                wisp_id: Some(cid),
                type_: concept_type.clone(),
                entries: user_entries.clone(),
                sections: sections
                    .iter()
                    .map(|s| (s.level, s.heading.clone(), s.body.clone()))
                    .collect(),
                footnotes: sections.iter().flat_map(|s| s.footnotes.clone()).collect(),
                source_ids: parsed_sources(&fm)
                    .into_iter()
                    .map(|(s, _)| s)
                    .chain(if is_new {
                        vec![]
                    } else {
                        Hot::sources_of(conn, cid)?
                            .into_iter()
                            .map(|s| s.source_id)
                            .collect()
                    })
                    .collect(),
                status: parsed_status(&fm),
                generated: None,
                verified: vec![],
                sources: vec![],
                retracted: None,
                line_ending: p.file.line_ending.to_string(),
            });
            let _ = &p.text;
        }

        // Missing files → retract (concepts) or retract (assets). A concept whose file moved is not missing.
        let present: HashSet<ConceptId> = decisions
            .iter()
            .filter_map(|d| match d {
                ConceptDecision::Known { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        for rel in &missing {
            if let Some(rec) = idx.files.get(rel) {
                match (
                    rec.kind.as_str(),
                    rec.id.as_deref().and_then(|s| s.parse::<ConceptId>().ok()),
                ) {
                    ("concept", Some(cid)) if present.contains(&cid) => {}
                    ("concept", Some(cid)) if Hot::concept(conn, cid)?.is_some() => {
                        let cstate = Hot::concept(conn, cid)?.unwrap();
                        let mut op = PlannedOp::new(
                            Op::Retract,
                            Target::Concept { concept: cid },
                            cstate.scope,
                        )
                        .with_precondition(Precondition::ConceptLive(cid))
                        .with_path(rel);
                        op.reason = Some(opts.reason.unwrap_or(Reason::RemovedFromTree));
                        op.note = opts.note.clone();
                        op.policy = PolicyFacts {
                            concept_type: cstate.type_.clone(),
                            ..Default::default()
                        };
                        ops.push(op);
                    }
                    ("asset", Some(_)) => {
                        if let Some(a) = Hot::asset_by_path(conn, rel)? {
                            let mut op = PlannedOp::new(
                                Op::Retract,
                                Target::Asset { asset: a.asset },
                                a.scope,
                            )
                            .with_path(rel);
                            op.reason = Some(opts.reason.unwrap_or(Reason::RemovedFromTree));
                            op.precondition = Precondition::AssetVersion {
                                asset: a.asset,
                                version: a.version,
                            };
                            ops.push(op);
                        }
                    }
                    _ => {}
                }
            }
        }

        // Assets.
        for (rel, bytes) in &assets_in {
            match Hot::asset_by_path(conn, rel)? {
                Some(a) => {
                    if a.bytes != *bytes {
                        status.changed.push(rel.clone());
                        ops.push(
                            PlannedOp::new(
                                Op::UpdateAsset,
                                Target::Asset { asset: a.asset },
                                a.scope,
                            )
                            .with_payload(PayloadKind::AssetBytes, encode_asset(rel, bytes))
                            .with_precondition(Precondition::AssetVersion {
                                asset: a.asset,
                                version: idx.assets.get(rel).map(|(_, v)| *v).unwrap_or(a.version),
                            })
                            .with_path(rel),
                        );
                    }
                }
                None => {
                    status.new.push(rel.clone());
                    let aid = AssetId::new();
                    let scope = ScopeId::new();
                    ops.push(
                        PlannedOp::new(Op::CreateAsset, Target::Asset { asset: aid }, scope)
                            .with_new_scope(ScopeKind::Asset { asset: aid })
                            .with_payload(PayloadKind::AssetBytes, encode_asset(rel, bytes))
                            .with_precondition(Precondition::PathFree(rel.clone()))
                            .with_path(rel),
                    );
                }
            }
        }

        // Validate (§9 step 7).
        let mut validation = Vec::new();
        for v in &views {
            validation.extend(repo.profile.validate(v));
        }
        Ok(Plan {
            ops,
            status,
            explanations,
            validation,
            base: idx.head,
        })
    })();
    repo.hots.insert(ref_name.clone(), hot);
    result
}

/// Fold separator spans (blank lines, free comments) into the preceding entry so round-trips are lossless.
fn fold_entries(fm: &wisp_parse::Frontmatter) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut leading = String::new();
    for item in &fm.items {
        match item {
            wisp_parse::FmItem::Entry { key, raw } => {
                let mut r = std::mem::take(&mut leading);
                r.push_str(raw);
                out.push((key.clone(), r));
            }
            wisp_parse::FmItem::Separator(sep) => match out.last_mut() {
                Some((_, r)) => r.push_str(sep),
                None => leading.push_str(sep),
            },
        }
    }
    out
}

/// The sections of a concept as the author last saw them: the base versions from `checkout.idx`,
/// read from the cold tier; the hot tier when a section has no base record.
fn base_sections(
    repo: &Repository,
    conn: &rusqlite::Connection,
    cid: ConceptId,
    base: Option<&checkout::ConceptBase>,
) -> Result<Vec<wisp_store::SectionState>> {
    let hot_now = Hot::sections_of(conn, cid)?;
    let Some(b) = base else { return Ok(hot_now) };
    let mut out = Vec::new();
    for (i, (sid, version)) in b.sections.iter().enumerate() {
        let scope = hot_now
            .iter()
            .find(|s| s.section == *sid)
            .map(|s| s.scope)
            .or_else(|| {
                Ledger::scope_of_section(repo.ledger.conn(), *sid)
                    .ok()
                    .flatten()
                    .map(|s| s.id)
            });
        let Some(scope) = scope else { continue };
        let Some(ch) = Ledger::get_change(repo.ledger.conn(), *version)? else {
            continue;
        };
        let Some(pr) = &ch.payload else { continue };
        let Some(pt) = repo.read_payload(ch.scope, pr)? else {
            continue;
        };
        let sb = SectionBody::decode(&pt)?;
        out.push(wisp_store::SectionState {
            concept: cid,
            section: *sid,
            idx: i as i64,
            level: sb.level,
            heading: sb.heading,
            heading_line: sb.heading_line,
            body: sb.body,
            scope,
            version: *version,
        });
    }
    Ok(out)
}

fn base_entries(
    repo: &Repository,
    conn: &rusqlite::Connection,
    cid: ConceptId,
    base: Option<&checkout::ConceptBase>,
) -> Result<Vec<wisp_store::FrontmatterState>> {
    let hot_now = Hot::frontmatter_of(conn, cid)?;
    let Some(b) = base else { return Ok(hot_now) };
    let mut out = Vec::new();
    for (i, (key, version)) in b.entries.iter().enumerate() {
        let Some(ch) = Ledger::get_change(repo.ledger.conn(), *version)? else {
            continue;
        };
        let Some(pr) = &ch.payload else { continue };
        let Some(pt) = repo.read_payload(ch.scope, pr)? else {
            continue;
        };
        let fp = decode_frontmatter(&pt)?;
        out.push(wisp_store::FrontmatterState {
            idx: i as i64,
            key: key.clone(),
            raw: fp.raw,
            managed: false,
            version: *version,
        });
    }
    Ok(out)
}

fn is_known_section(conn: &rusqlite::Connection, id: SectionId) -> bool {
    Hot::section(conn, id).ok().flatten().is_some()
}

/// Keyed commitment to a frontmatter key name under the concept scope (Target::FrontmatterKey).
fn key_commit(repo: &Repository, scope: ScopeId, key: &str) -> Commit {
    match repo.keystore.get(scope) {
        Ok(Some(k)) => wisp_crypto::commitment(
            &k.commit_key(repo.repo_id()),
            PayloadKind::FrontmatterEntry,
            key.as_bytes(),
        ),
        _ => {
            // New scope whose key is minted at commit time: commit to the key name under a zero key.
            // (The target is informational; the entry payload carries the key name authoritatively.)
            wisp_crypto::commitment(&[0u8; 32], PayloadKind::FrontmatterEntry, key.as_bytes())
        }
    }
}

fn parsed_sources(fm: &wisp_parse::Frontmatter) -> Vec<(String, serde_json::Value)> {
    let Some(raw) = fm.get("sources") else {
        return vec![];
    };
    let Some(serde_yaml::Value::Sequence(items)) = wisp_parse::frontmatter::entry_value(raw) else {
        return vec![];
    };
    items
        .into_iter()
        .filter_map(|v| {
            let j: serde_json::Value = serde_json::to_value(&v).ok()?;
            let id = j.get("id").and_then(|x| x.as_str())?.to_string();
            Some((id, j))
        })
        .collect()
}

fn parsed_status(fm: &wisp_parse::Frontmatter) -> Option<wisp_model::Status> {
    fm.get("status")
        .and_then(wisp_parse::frontmatter::entry_string)
        .and_then(|s| wisp_model::Status::parse(&s))
}

/// `wisp record` (§9).
pub fn record(repo: &mut Repository, opts: &RecordOptions) -> Result<RecordResult> {
    let p = plan(repo, opts)?;
    let ref_name = p.status.ref_name.clone();
    // Step 6: generated files.
    if !p.status.generated_modified.is_empty() && !opts.adopt_generated {
        return Err(WispError::refused(
            "generated files were edited; pass --adopt-generated to discard those edits",
        )
        .with_details(json!({ "generated_modified": p.status.generated_modified })));
    }
    // Step 7: validation.
    let musts: Vec<&wisp_profile::Finding> = p
        .validation
        .iter()
        .filter(|f| f.level == Level::Must)
        .collect();
    if !musts.is_empty() && !opts.allow_nonconformant {
        return Err(WispError::validation(
            "profile validation failed",
            serde_json::to_value(&p.validation)?,
        ));
    }
    if !p.status.nonconformant.is_empty() && !opts.allow_nonconformant {
        return Err(WispError::validation(
            "non-conformant files; pass --allow-nonconformant to record them",
            json!(p
                .status
                .nonconformant
                .iter()
                .map(|f| json!({ "path": f, "code": "frontmatter" }))
                .collect::<Vec<_>>()),
        ));
    }
    let warnings: Vec<Value> = p
        .validation
        .iter()
        .filter(|f| f.level == Level::Should || opts.allow_nonconformant)
        .map(|f| serde_json::to_value(f).unwrap_or(Value::Null))
        .chain(p.status.warnings.iter().map(|w| json!({ "warning": w })))
        .collect();
    if p.ops.is_empty() || opts.dry_run {
        return Ok(RecordResult {
            changeset: None,
            seq: None,
            ops: p.ops.len(),
            status: p.status,
            explanations: p.explanations,
            warnings,
        });
    }
    let actor = opts.actor.clone().unwrap_or_else(|| repo.default_actor());
    let mut req = CommitRequest::new(&ref_name, &actor);
    req.base = p.base;
    req.message = opts.message.clone();
    req.inputs = opts.inputs.clone();
    req.ops = p.ops;
    let n_ops = req.ops.len();
    let out: CommitOutcome = crate::commit::commit(repo, req)?;
    // Steps 10–11: rewrite the tree (anchors, wisp_id, managed keys, generated views) and record the index.
    let root = repo.root.clone();
    let _lock = checkout::WorktreeLock::acquire(&repo.wisp_dir)?;
    let idx = checkout::write_tree(repo, &ref_name, &root, false, false, true)?;
    idx.save(&checkout::checkout_idx_path(repo, &root))?;
    if opts.resolve_merge {
        match merge::read_merge_state(repo)? {
            Some(state) => {
                let into = state["into"].as_str().unwrap_or_default();
                if into != ref_name {
                    return Err(WispError::usage(format!(
                        "merge in progress on ref {into}; record --resolve on that ref or `wisp merge --abort`"
                    )));
                }
                merge::clear_merge_state(repo)?;
            }
            None => {
                return Err(WispError::usage(
                    "record --resolve requires `.wisp/merge.json` from a conflicted merge",
                ));
            }
        }
    }
    Ok(RecordResult {
        changeset: Some(out.changeset.to_string()),
        seq: Some(out.seq),
        ops: n_ops,
        status: p.status,
        explanations: p.explanations,
        warnings,
    })
}

/// Unrecorded edits under a managed checkout root (primary or linked worktree).
pub fn unrecorded_edits_at(repo: &Repository, ref_name: &str, tree_root: &Path) -> Result<Status> {
    let idx_path = checkout::checkout_idx_path(repo, tree_root);
    let idx = CheckoutIndex::load(&idx_path)?.unwrap_or_default();
    let (scanned, generated_modified, missing) = scan(repo, tree_root, &idx, false)?;
    let head = Ledger::head(repo.ledger.conn(), ref_name)?;
    let mut status = Status {
        ref_name: ref_name.to_string(),
        head: head.map(|h| h.to_string()),
        ..Default::default()
    };
    status.generated_modified = generated_modified;
    status.missing = missing;
    for s in scanned {
        if !s.changed {
            continue;
        }
        if idx.files.contains_key(&s.rel) {
            status.changed.push(s.rel);
        } else {
            status.new.push(s.rel);
        }
    }
    Ok(status)
}

/// `wisp status`.
pub fn status(repo: &mut Repository, ref_name: Option<&str>) -> Result<Status> {
    let opts = RecordOptions {
        ref_name: ref_name.map(|s| s.to_string()),
        dry_run: true,
        ..Default::default()
    };
    Ok(plan(repo, &opts)?.status)
}

/// `wisp diff [--explain]`: a section-level description of what `record` would commit.
pub fn diff(
    repo: &mut Repository,
    ref_name: Option<&str>,
    explain: bool,
    path_filter: Option<&str>,
) -> Result<Value> {
    let opts = RecordOptions {
        ref_name: ref_name.map(|s| s.to_string()),
        dry_run: true,
        explain,
        ..Default::default()
    };
    let p = plan(repo, &opts)?;
    let mut by_path: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for op in &p.ops {
        let path = op.path.clone().unwrap_or_default();
        if let Some(f) = path_filter {
            if !path.starts_with(f) {
                continue;
            }
        }
        let mut v = json!({ "op": op.op.as_str() });
        match &op.target {
            Target::Section { section, .. } => v["section"] = json!(section.to_string()),
            Target::FrontmatterKey { .. } => {
                if let Some(k) = &op.policy.key {
                    v["key"] = json!(k);
                }
            }
            Target::Source { source, .. } => v["source"] = json!(source),
            _ => {}
        }
        if let Some(h) = &op.policy.section_heading {
            v["heading"] = json!(h);
        }
        if let Some(l) = &op.lineage {
            v["lineage"] = serde_json::to_value(l).unwrap_or(Value::Null);
        }
        by_path.entry(path).or_default().push(v);
    }
    Ok(
        json!({ "ref": p.status.ref_name, "head": p.status.head, "changes": by_path, "explain": p.explanations, "warnings": p.status.warnings }),
    )
}

fn change_path(hot: &Hot, c: &Change) -> String {
    if let Target::Asset { asset } = &c.target {
        return Hot::asset(hot.conn(), *asset)
            .ok()
            .flatten()
            .map(|a| a.path)
            .unwrap_or_else(|| format!("asset:{asset}"));
    }
    if let Some(cp) = c.target.concept() {
        if let Ok(Some(st)) = Hot::concept(hot.conn(), cp) {
            return st.path;
        }
        if let Ok(Some(r)) = Hot::retracted(hot.conn(), cp) {
            return r.0;
        }
    }
    String::new()
}

/// `wisp diff --from <cs> --to <cs>`: section-level ops between two changesets on the same line.
pub fn diff_between_changesets(
    repo: &mut Repository,
    ref_name: &str,
    from: ChangesetId,
    to: ChangesetId,
    path_filter: Option<&str>,
) -> Result<Value> {
    if from == to {
        return Ok(json!({
            "from": from.to_string(),
            "to": to.to_string(),
            "changes": Value::Object(Default::default()),
        }));
    }
    let cs_ids = {
        let conn = repo.ledger.conn();
        if Ledger::get_changeset(conn, from)?.is_none() {
            return Err(WispError::not_found(format!("no changeset {from}")));
        }
        if Ledger::get_changeset(conn, to)?.is_none() {
            return Err(WispError::not_found(format!("no changeset {to}")));
        }
        if !merge::is_ancestor(conn, from, to)? {
            return Err(WispError::usage(format!(
                "{from} is not an ancestor of {to}"
            )));
        }
        merge::changesets_since(conn, to, from)
    };
    repo.hot(ref_name)?;
    let conn = repo.ledger.conn();
    let hot = repo.hots.remove(ref_name).expect("hot");
    let mut by_path: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for cs_id in cs_ids {
        for c in Ledger::changes_of(conn, cs_id)? {
            let path = change_path(&hot, &c);
            if let Some(f) = path_filter {
                if !path.starts_with(f) {
                    continue;
                }
            }
            let mut v = json!({ "op": c.op.as_str(), "changeset": cs_id.to_string() });
            if let Some(s) = c.target.section() {
                v["section"] = json!(s.to_string());
            }
            if let Target::Source { source, .. } = &c.target {
                v["source"] = json!(source);
            }
            if let Some(l) = &c.lineage {
                v["lineage"] = serde_json::to_value(l).unwrap_or(Value::Null);
            }
            by_path.entry(path).or_default().push(v);
        }
    }
    repo.hots.insert(ref_name.to_string(), hot);
    Ok(json!({
        "from": from.to_string(),
        "to": to.to_string(),
        "changes": by_path,
    }))
}
