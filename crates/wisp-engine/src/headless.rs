//! Headless writes (§11.3): commit directly to a ref without touching any file first.

use crate::checkout;
use crate::commit::{commit, Assertion, CommitRequest, PlannedOp, PolicyFacts, Precondition};
use crate::erase::resolve_target;
use crate::payloads::*;
use crate::repo::{Repository, MAIN_REF};
use serde::Deserialize;
use serde_json::{json, Value};
use wisp_model::*;
use wisp_store::{Hot, Ledger};

/// The `apply` batch format (§11.3).
#[derive(Clone, Debug, Deserialize)]
pub struct ApplyBatch {
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub base: Option<ChangesetId>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    pub ops: Vec<ApplyOp>,
    #[serde(default)]
    pub inputs: Vec<ApplyInput>,
    #[serde(default)]
    pub assert: Vec<ApplyAssert>,
    #[serde(default)]
    pub assert_head: Option<ChangesetId>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApplyInput {
    pub concept: String,
    #[serde(default)]
    pub section: Option<SectionId>,
    #[serde(default)]
    pub at: Option<ChangesetId>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApplyAssert {
    pub path: String,
    #[serde(default)]
    pub section: Option<SectionId>,
    pub version: ChangeId,
}

/// `base_version` is a change ID, `"absent"`, or `"any"`.
#[derive(Clone, Debug, Deserialize)]
pub struct ApplyOp {
    pub op: String,
    pub path: String,
    #[serde(default)]
    pub section: Option<SectionId>,
    #[serde(default)]
    pub base_version: Option<String>,
    #[serde(default)]
    pub heading: Option<String>,
    #[serde(default)]
    pub level: Option<u8>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub frontmatter: Option<Value>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub split_from: Option<SectionId>,
    #[serde(default)]
    pub merged_from: Option<Vec<SectionId>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Version(ChangeId),
    Absent,
    Any,
}

pub fn parse_base(s: Option<&str>) -> Result<Option<Base>> {
    match s {
        None => Ok(None),
        Some("absent") => Ok(Some(Base::Absent)),
        Some("any") => Ok(Some(Base::Any)),
        Some(v) => {
            Ok(Some(Base::Version(v.parse().map_err(|_| {
                WispError::usage(format!("bad base version {v:?}"))
            })?)))
        }
    }
}

fn heading_line_for(level: u8, heading: &str, le: &str) -> String {
    format!("{} {}{}", "#".repeat(level.max(1) as usize), heading, le)
}

fn ensure_trailing_newline(mut s: String, le: &str) -> String {
    if !s.is_empty() && !s.ends_with('\n') {
        s.push_str(le);
    }
    s
}

/// `wisp cat <path>[#section] --json`: content, version, and the ref head (how agents obtain base versions).
pub fn cat(
    repo: &mut Repository,
    ref_name: &str,
    spec: &str,
    version: Option<ChangeId>,
) -> Result<Value> {
    let head = repo.head(ref_name)?;
    if let Some(v) = version {
        // Historical read from the cold tier.
        let ch = Ledger::get_change(repo.ledger.conn(), v)?
            .ok_or_else(|| WispError::not_found(format!("no change {v}")))?;
        let pr = ch
            .payload
            .ok_or_else(|| WispError::not_found("change has no content"))?;
        let pt = repo
            .read_payload(ch.scope, &pr)?
            .ok_or_else(|| WispError::key_unavailable("content erased"))?;
        let content = match ch.op {
            Op::AddSection | Op::UpdateSection | Op::Split | Op::MoveSection => {
                let sb = SectionBody::decode(&pt)?;
                json!({ "heading": sb.heading, "level": sb.level, "content": sb.body })
            }
            _ => json!({ "content": String::from_utf8_lossy(&pt) }),
        };
        return Ok(
            json!({ "ref": ref_name, "head": head.map(|h| h.to_string()), "version": v.to_string(), "op": ch.op.as_str(), "data": content }),
        );
    }
    let (c, section) = resolve_target(repo, ref_name, spec)?;
    let conn = repo.hot(ref_name)?.conn();
    match section {
        Some(s) => {
            let st = Hot::section(conn, s)?.ok_or_else(|| WispError::not_found("section"))?;
            Ok(json!({
                "ref": ref_name, "head": head.map(|h| h.to_string()),
                "path": c.path, "concept": c.concept.to_string(), "section": s.to_string(),
                "version": st.version.to_string(), "heading": st.heading, "level": st.level, "content": st.body
            }))
        }
        None => {
            let sections: Vec<Value> = Hot::sections_of(conn, c.concept)?
                .into_iter()
                .map(|s| json!({ "section": s.section.to_string(), "version": s.version.to_string(), "heading": s.heading, "level": s.level, "content": s.body }))
                .collect();
            let entries: Vec<Value> = Hot::frontmatter_of(conn, c.concept)?
                .into_iter()
                .map(|e| json!({ "key": e.key, "raw": e.raw, "version": e.version.to_string() }))
                .collect();
            Ok(json!({
                "ref": ref_name, "head": head.map(|h| h.to_string()),
                "path": c.path, "concept": c.concept.to_string(), "version": c.version.to_string(),
                "content_version": c.content_version.map(|v| v.to_string()),
                "type": c.type_, "status": c.status.map(|s| s.as_str()),
                "frontmatter": entries, "sections": sections
            }))
        }
    }
}

/// Plan one `apply` operation against the ref's hot tier.
fn plan_op(repo: &mut Repository, ref_name: &str, o: &ApplyOp, le: &str) -> Result<PlannedOp> {
    let base = parse_base(o.base_version.as_deref())?;
    let op = Op::parse(&o.op).ok_or_else(|| WispError::usage(format!("unknown op {:?}", o.op)))?;
    let pre_section = |section: SectionId, cur: ChangeId| -> Precondition {
        match base {
            Some(Base::Version(v)) => Precondition::SectionVersion {
                section,
                version: v,
            },
            Some(Base::Any) => Precondition::Any,
            Some(Base::Absent) | None => Precondition::SectionVersion {
                section,
                version: cur,
            },
        }
    };
    match op {
        Op::CreateConcept => {
            let path = concept_path_of(&o.path);
            let cid = ConceptId::new();
            let cscope = ScopeId::new();
            repo.new_scope_key(cscope)?;
            let type_ = o
                .r#type
                .clone()
                .ok_or_else(|| WispError::usage("create requires --type"))?;
            let mut p = PlannedOp::new(Op::CreateConcept, Target::Concept { concept: cid }, cscope)
                .with_new_scope(ScopeKind::Concept { concept: cid })
                .with_payload(PayloadKind::Path, path.as_bytes().to_vec())
                .with_precondition(Precondition::PathFree(path.clone()))
                .with_path(format!("{path}.md"));
            p.policy = PolicyFacts {
                concept_type: Some(type_),
                ..Default::default()
            };
            Ok(p)
        }
        Op::AddSection | Op::Split => {
            let (c, _) = resolve_target(repo, ref_name, &o.path)?;
            let heading = o
                .heading
                .clone()
                .ok_or_else(|| WispError::usage("add-section requires --heading"))?;
            let level = o.level.unwrap_or(2);
            let after = match o.after.as_deref() {
                None | Some("^") | Some("") => None,
                Some(k) => Some(
                    k.parse::<SectionId>()
                        .map_err(|_| WispError::usage(format!("bad --after {k:?}")))?,
                ),
            };
            let body = ensure_trailing_newline(o.content.clone().unwrap_or_default(), le);
            let sid = SectionId::new();
            let sscope = ScopeId::new();
            let sb = SectionBody {
                level,
                heading: heading.clone(),
                heading_line: heading_line_for(level, &heading, le),
                body,
                after,
                at_start: false,
            };
            let mut p = PlannedOp::new(
                op,
                Target::Section {
                    concept: c.concept,
                    section: sid,
                },
                sscope,
            )
            .with_new_scope(ScopeKind::Section {
                concept: c.concept,
                section: sid,
            })
            .with_payload(PayloadKind::Section, sb.encode())
            .with_precondition(Precondition::ConceptLive(c.concept))
            .with_path(format!("{}.md", c.path));
            p.policy = PolicyFacts {
                concept_type: c.type_.clone(),
                section_heading: Some(heading),
                ..Default::default()
            };
            p.note = o.note.clone();
            if let Some(from) = o.split_from {
                p.lineage = Some(Lineage::SplitFrom { section: from });
            }
            Ok(p)
        }
        Op::UpdateSection => {
            let spec = match o.section {
                Some(s) => format!("{}#{}", o.path, s),
                None => o.path.clone(),
            };
            let (c, section) = resolve_target(repo, ref_name, &spec)?;
            let s = section.ok_or_else(|| WispError::usage("update requires <path>#<section>"))?;
            let st = Hot::section(repo.hot(ref_name)?.conn(), s)?
                .ok_or_else(|| WispError::not_found("section"))?;
            let level = o.level.unwrap_or(st.level);
            let heading = o.heading.clone().unwrap_or_else(|| st.heading.clone());
            let heading_line = if o.heading.is_some() || o.level.is_some() {
                heading_line_for(level, &heading, le)
            } else {
                st.heading_line.clone()
            };
            let body =
                ensure_trailing_newline(o.content.clone().unwrap_or_else(|| st.body.clone()), le);
            let sb = SectionBody {
                level,
                heading: heading.clone(),
                heading_line,
                body,
                after: None,
                at_start: false,
            };
            let mut p = PlannedOp::new(
                Op::UpdateSection,
                Target::Section {
                    concept: c.concept,
                    section: s,
                },
                st.scope,
            )
            .with_payload(PayloadKind::Section, sb.encode())
            .with_precondition(pre_section(s, st.version))
            .with_path(format!("{}.md", c.path));
            p.policy = PolicyFacts {
                concept_type: c.type_.clone(),
                section_heading: Some(st.heading.clone()),
                ..Default::default()
            };
            p.note = o.note.clone();
            if let Some(m) = &o.merged_from {
                p.lineage = Some(Lineage::MergedFrom {
                    sections: m.clone(),
                });
            }
            Ok(p)
        }
        Op::RemoveSection => {
            let spec = match o.section {
                Some(s) => format!("{}#{}", o.path, s),
                None => o.path.clone(),
            };
            let (c, section) = resolve_target(repo, ref_name, &spec)?;
            let s = section
                .ok_or_else(|| WispError::usage("remove-section requires <path>#<section>"))?;
            let st = Hot::section(repo.hot(ref_name)?.conn(), s)?
                .ok_or_else(|| WispError::not_found("section"))?;
            let mut p = PlannedOp::new(
                Op::RemoveSection,
                Target::Section {
                    concept: c.concept,
                    section: s,
                },
                st.scope,
            )
            .with_precondition(pre_section(s, st.version))
            .with_path(format!("{}.md", c.path));
            p.policy = PolicyFacts {
                concept_type: c.type_.clone(),
                section_heading: Some(st.heading.clone()),
                ..Default::default()
            };
            p.note = o.note.clone();
            Ok(p)
        }
        Op::SetKey | Op::UnsetKey => {
            let (c, _) = resolve_target(repo, ref_name, &o.path)?;
            let key = o
                .key
                .clone()
                .ok_or_else(|| WispError::usage("set/unset require a key"))?;
            if repo.profile.managed_keys().contains(&key.as_str()) || key == repo.identity_key() {
                return Err(WispError::refused(format!(
                    "`{key}` is a managed key; it is rendered from the log"
                )));
            }
            let existing = Hot::frontmatter_entry(repo.hot(ref_name)?.conn(), c.concept, &key)?;
            let pre = match base {
                Some(Base::Version(v)) => Precondition::EntryVersion {
                    concept: c.concept,
                    key: key.clone(),
                    version: Some(v),
                },
                Some(Base::Absent) => Precondition::EntryVersion {
                    concept: c.concept,
                    key: key.clone(),
                    version: None,
                },
                Some(Base::Any) => Precondition::Any,
                None => Precondition::EntryVersion {
                    concept: c.concept,
                    key: key.clone(),
                    version: existing.as_ref().map(|e| e.version),
                },
            };
            let raw = if op == Op::SetKey {
                let v = o
                    .value
                    .clone()
                    .ok_or_else(|| WispError::usage("set requires a value"))?;
                let yv: serde_yaml::Value = serde_yaml::to_value(&v)
                    .map_err(|e| WispError::usage(format!("value: {e}")))?;
                wisp_parse::frontmatter::format_entry(&key, &yv, le)
            } else {
                String::new()
            };
            let key_commit = {
                let k = repo.scope_key(c.scope)?;
                wisp_crypto::commitment(
                    &k.commit_key(repo.repo_id()),
                    PayloadKind::FrontmatterEntry,
                    key.as_bytes(),
                )
            };
            let mut p = PlannedOp::new(
                op,
                Target::FrontmatterKey {
                    concept: c.concept,
                    key_commit,
                },
                c.scope,
            )
            .with_payload(
                PayloadKind::FrontmatterEntry,
                encode_frontmatter(&key, &raw),
            )
            .with_precondition(pre)
            .with_path(format!("{}.md", c.path));
            p.policy = PolicyFacts {
                concept_type: c.type_.clone(),
                key: Some(key),
                ..Default::default()
            };
            p.note = o.note.clone();
            Ok(p)
        }
        Op::SetStatus => {
            let (c, _) = resolve_target(repo, ref_name, &o.path)?;
            let v = o
                .value
                .as_ref()
                .and_then(|v| v.as_str())
                .ok_or_else(|| WispError::usage("set_status requires a value"))?;
            let st = Status::parse(v)
                .ok_or_else(|| WispError::usage("status must be draft, stable, or deprecated"))?;
            let mut p = PlannedOp::new(
                Op::SetStatus,
                Target::Concept { concept: c.concept },
                c.scope,
            )
            .with_payload(PayloadKind::Status, st.as_str().as_bytes().to_vec())
            .with_precondition(Precondition::ConceptLive(c.concept))
            .with_path(format!("{}.md", c.path));
            p.policy = PolicyFacts {
                concept_type: c.type_.clone(),
                ..Default::default()
            };
            Ok(p)
        }
        Op::Verify => {
            let (c, _) = resolve_target(repo, ref_name, &o.path)?;
            let cv = c.content_version.unwrap_or(c.version);
            let mut p = PlannedOp::new(Op::Verify, Target::Concept { concept: c.concept }, c.scope)
                .with_precondition(match base {
                    Some(Base::Version(v)) => Precondition::ContentVersion {
                        concept: c.concept,
                        version: v,
                    },
                    _ => Precondition::ContentVersion {
                        concept: c.concept,
                        version: cv,
                    },
                })
                .with_path(format!("{}.md", c.path));
            p.policy = PolicyFacts {
                concept_type: c.type_.clone(),
                ..Default::default()
            };
            Ok(p)
        }
        Op::SourcePut | Op::SourceRemove => {
            let (c, _) = resolve_target(repo, ref_name, &o.path)?;
            let id = o
                .key
                .clone()
                .or_else(|| {
                    o.value
                        .as_ref()
                        .and_then(|v| v.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .ok_or_else(|| WispError::usage("source ops need an id"))?;
            let existing = Hot::sources_of(repo.hot(ref_name)?.conn(), c.concept)?
                .into_iter()
                .find(|s| s.source_id == id);
            let mut p = PlannedOp::new(
                op,
                Target::Source {
                    concept: c.concept,
                    source: id.clone(),
                },
                c.scope,
            )
            .with_precondition(Precondition::SourceVersion {
                concept: c.concept,
                id: id.clone(),
                version: existing.map(|e| e.version),
            })
            .with_path(format!("{}.md", c.path));
            if op == Op::SourcePut {
                let mut rec = o.value.clone().unwrap_or(json!({}));
                if rec.get("id").is_none() {
                    rec["id"] = json!(id);
                }
                p.payload = Some((PayloadKind::SourceRecord, encode_source(&id, &rec)));
            }
            p.policy = PolicyFacts {
                concept_type: c.type_.clone(),
                ..Default::default()
            };
            Ok(p)
        }
        other => Err(WispError::usage(format!(
            "{} is not a headless operation",
            other.as_str()
        ))),
    }
}

/// `wisp apply`: one batch, one changeset. Every other headless command is shorthand for a one-op batch.
pub fn apply(repo: &mut Repository, batch: ApplyBatch, default_ref: Option<&str>) -> Result<Value> {
    let ref_name = batch
        .r#ref
        .clone()
        .or_else(|| default_ref.map(|s| s.to_string()))
        .unwrap_or_else(|| MAIN_REF.to_string());
    let actor = batch.actor.clone().unwrap_or_else(|| repo.default_actor());
    let le = "\n";
    let mut ops = Vec::with_capacity(batch.ops.len());
    for o in &batch.ops {
        ops.push(plan_op(repo, &ref_name, o, le)?);
    }
    // A `create` may be followed by frontmatter and content in the same batch.
    let mut extra: Vec<PlannedOp> = Vec::new();
    for (o, p) in batch.ops.iter().zip(ops.iter()) {
        if p.op == Op::CreateConcept {
            let Target::Concept { concept } = &p.target else {
                continue;
            };
            let cscope = p.scope;
            let mut fm = serde_json::Map::new();
            if let Some(t) = &o.r#type {
                fm.insert("type".into(), json!(t));
            }
            if let Some(Value::Object(m)) = &o.frontmatter {
                for (k, v) in m {
                    fm.insert(k.clone(), v.clone());
                }
            }
            for (k, v) in fm {
                if repo.profile.managed_keys().contains(&k.as_str()) || k == repo.identity_key() {
                    continue;
                }
                let yv: serde_yaml::Value = serde_yaml::to_value(&v)
                    .map_err(|e| WispError::usage(format!("value: {e}")))?;
                let raw = wisp_parse::frontmatter::format_entry(&k, &yv, le);
                let key_commit = {
                    let key = repo.scope_key(cscope)?;
                    wisp_crypto::commitment(
                        &key.commit_key(repo.repo_id()),
                        PayloadKind::FrontmatterEntry,
                        k.as_bytes(),
                    )
                };
                let mut e = PlannedOp::new(
                    Op::SetKey,
                    Target::FrontmatterKey {
                        concept: *concept,
                        key_commit,
                    },
                    cscope,
                )
                .with_payload(PayloadKind::FrontmatterEntry, encode_frontmatter(&k, &raw))
                .with_path(p.path.clone().unwrap_or_default());
                e.policy = PolicyFacts {
                    concept_type: o.r#type.clone(),
                    key: Some(k),
                    ..Default::default()
                };
                extra.push(e);
            }
            // The preamble, with the given content.
            let sid = SectionId::new();
            let sscope = ScopeId::new();
            let body = ensure_trailing_newline(o.content.clone().unwrap_or_default(), le);
            let sb = SectionBody {
                level: 0,
                heading: String::new(),
                heading_line: String::new(),
                body,
                after: None,
                at_start: true,
            };
            extra.push(
                PlannedOp::new(
                    Op::AddSection,
                    Target::Section {
                        concept: *concept,
                        section: sid,
                    },
                    sscope,
                )
                .with_new_scope(ScopeKind::Section {
                    concept: *concept,
                    section: sid,
                })
                .with_payload(PayloadKind::Section, sb.encode())
                .with_path(p.path.clone().unwrap_or_default()),
            );
        }
    }
    ops.extend(extra);

    let mut req = CommitRequest::new(&ref_name, &actor);
    req.base = batch.base;
    req.message = batch.message.clone();
    req.assert_head = batch.assert_head;
    for i in &batch.inputs {
        let (c, _) = resolve_target(repo, &ref_name, &i.concept)?;
        req.inputs.push(InputRef::Concept {
            concept: c.concept,
            section: i.section,
            at: i.at,
        });
    }
    for a in &batch.assert {
        let (c, _) = resolve_target(repo, &ref_name, &a.path)?;
        match a.section {
            Some(s) => req.asserts.push(Assertion::Section {
                section: s,
                version: a.version,
                path: a.path.clone(),
            }),
            None => req.asserts.push(Assertion::Concept {
                concept: c.concept,
                version: a.version,
                path: a.path.clone(),
            }),
        }
    }
    req.ops = ops;
    let out = commit(repo, req)?;
    // Effect on working trees: rewrite files whose on-disk content still matches their base (§11.3).
    let rewritten = refresh_worktree_after_headless(repo, &ref_name)?;
    let versions: Vec<Value> = out
        .changes
        .iter()
        .filter(|c| c.op != Op::RegisterActor)
        .map(|c| {
            let mut v = json!({ "op": c.op.as_str(), "version": c.id.to_string() });
            if let Some(s) = c.target.section() {
                v["section"] = json!(s.to_string());
            }
            if let Some(cp) = c.target.concept() {
                v["concept"] = json!(cp.to_string());
            }
            v
        })
        .collect();
    Ok(json!({
        "ref": ref_name,
        "changeset": out.changeset.to_string(),
        "seq": out.seq,
        "head": out.changeset.to_string(),
        "changes": versions,
        "worktree_files_rewritten": rewritten,
    }))
}

/// After a headless commit to a ref checked out in the main worktree, rewrite affected files whose
/// on-disk content still matches `checkout.idx`; files with unrecorded edits keep their old base.
pub fn refresh_worktree_after_headless(
    repo: &mut Repository,
    ref_name: &str,
) -> Result<Vec<String>> {
    if ref_name != MAIN_REF {
        return Ok(vec![]);
    }
    let root = repo.root.clone();
    let _lock = checkout::WorktreeLock::acquire(&repo.wisp_dir)?;
    let idx_path = checkout::checkout_idx_path(repo, &root);
    let Some(old) = checkout::CheckoutIndex::load(&idx_path)? else {
        return Ok(vec![]);
    };
    // Which files have unrecorded edits?
    let mut dirty: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (rel, rec) in &old.files {
        if checkout::file_changed(&root.join(rel), rec) {
            dirty.insert(rel.clone());
        }
    }
    // Render the new view into a temp dir, then copy over only clean files.
    let tmp = repo
        .wisp_dir
        .join("cache")
        .join(format!("headless-{}", ChangeId::new()));
    std::fs::create_dir_all(&tmp)?;
    let fresh = checkout::write_tree(repo, ref_name, &tmp, false, false, false)?;
    let mut new_idx = old.clone();
    new_idx.head = fresh.head;
    let mut rewritten = Vec::new();
    for (rel, rec) in &fresh.files {
        if dirty.contains(rel) {
            continue;
        }
        let src = tmp.join(rel);
        let dst = root.join(rel);
        let bytes = std::fs::read(&src).map_err(|e| {
            let listing: Vec<String> = std::fs::read_dir(&tmp)
                .map(|rd| {
                    rd.flatten()
                        .map(|x| x.file_name().to_string_lossy().to_string())
                        .collect()
                })
                .unwrap_or_default();
            WispError::internal(format!(
                "read {}: {e}; tmp exists={} listing={:?} files={:?}",
                src.display(),
                tmp.exists(),
                listing,
                fresh.files.keys().collect::<Vec<_>>()
            ))
        })?;
        let same = std::fs::read(&dst).map(|b| b == bytes).unwrap_or(false);
        if !same {
            checkout::write_atomic(&dst, &bytes)?;
            rewritten.push(rel.clone());
        }
        new_idx.files.insert(
            rel.clone(),
            checkout::file_record(&dst, &rec.kind, rec.id.clone())?,
        );
    }
    // Base versions for clean concepts advance; dirty files keep their old base.
    for (cid, base) in &fresh.concepts {
        let rel = format!("{}.md", base.path);
        if !dirty.contains(&rel) {
            new_idx.concepts.insert(cid.clone(), base.clone());
        }
    }
    new_idx.assets = fresh.assets.clone();
    new_idx.save(&idx_path)?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(rewritten)
}

/// Convenience: a single-op batch.
pub fn single(
    repo: &mut Repository,
    ref_name: Option<&str>,
    actor: Option<String>,
    message: Option<String>,
    base: Option<ChangesetId>,
    op: ApplyOp,
) -> Result<Value> {
    let batch = ApplyBatch {
        r#ref: ref_name.map(|s| s.to_string()),
        base,
        actor,
        message,
        ops: vec![op],
        inputs: vec![],
        assert: vec![],
        assert_head: None,
    };
    apply(repo, batch, None)
}
