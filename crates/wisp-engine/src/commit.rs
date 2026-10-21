//! Optimistic commits (§11.2): prepare unlocked, validate per-entity preconditions at the head,
//! commit the ledger and the ref's hot tier inside one critical section.

use crate::manifest::{check_policy, PolicySubject};
use crate::repo::{PreparedPayload, Reader, Repository};
use rusqlite::Connection;
use serde_json::{json, Value};
use wisp_model::*;
use wisp_store::{ActorRow, Hot, Ledger};

/// What must hold at the head for an operation to commit (§11.2 precondition table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Precondition {
    None,
    /// Section exists at exactly this version.
    SectionVersion {
        section: SectionId,
        version: ChangeId,
    },
    /// Concept exists and is not retracted (add_section, set_status, create at path).
    ConceptLive(ConceptId),
    ConceptRetracted(ConceptId),
    /// Entry at this version, or absent when `version` is None.
    EntryVersion {
        concept: ConceptId,
        key: String,
        version: Option<ChangeId>,
    },
    SourceVersion {
        concept: ConceptId,
        id: String,
        version: Option<ChangeId>,
    },
    PathFree(String),
    SectionOrder {
        concept: ConceptId,
        order: Vec<SectionId>,
    },
    ContentVersion {
        concept: ConceptId,
        version: ChangeId,
    },
    AssetVersion {
        asset: AssetId,
        version: ChangeId,
    },
    /// `--base any`: last-writer-wins.
    Any,
}

#[derive(Clone, Debug, Default)]
pub struct PolicyFacts {
    pub concept_type: Option<String>,
    pub section_heading: Option<String>,
    pub key: Option<String>,
}

/// An operation planned in memory, before encryption.
#[derive(Clone, Debug)]
pub struct PlannedOp {
    pub op: Op,
    pub target: Target,
    pub scope: ScopeId,
    /// Set when `scope` is new and must be created (key minted, scope row inserted).
    pub new_scope: Option<ScopeKind>,
    pub payload: Option<(PayloadKind, Vec<u8>)>,
    pub note: Option<String>,
    pub inputs: Vec<InputRef>,
    pub lineage: Option<Lineage>,
    pub origin: Option<Origin>,
    pub reason: Option<Reason>,
    pub precondition: Precondition,
    /// Concept path, for conflict objects and reports.
    pub path: Option<String>,
    pub policy: PolicyFacts,
}

impl PlannedOp {
    pub fn new(op: Op, target: Target, scope: ScopeId) -> Self {
        PlannedOp {
            op,
            target,
            scope,
            new_scope: None,
            payload: None,
            note: None,
            inputs: vec![],
            lineage: None,
            origin: None,
            reason: None,
            precondition: Precondition::None,
            path: None,
            policy: PolicyFacts::default(),
        }
    }
    pub fn with_new_scope(mut self, kind: ScopeKind) -> Self {
        self.new_scope = Some(kind);
        self
    }
    pub fn with_payload(mut self, kind: PayloadKind, bytes: Vec<u8>) -> Self {
        self.payload = Some((kind, bytes));
        self
    }
    pub fn with_precondition(mut self, p: Precondition) -> Self {
        self.precondition = p;
        self
    }
    pub fn with_path(mut self, p: impl Into<String>) -> Self {
        self.path = Some(p.into());
        self
    }
}

#[derive(Clone, Debug)]
pub enum Assertion {
    Section {
        section: SectionId,
        version: ChangeId,
        path: String,
    },
    Concept {
        concept: ConceptId,
        version: ChangeId,
        path: String,
    },
}

#[derive(Clone, Debug)]
pub struct CommitRequest {
    pub ref_name: String,
    /// The head the author read. None means "no base": preconditions still apply individually.
    pub base: Option<ChangesetId>,
    pub actor: String,
    pub message: Option<String>,
    pub ops: Vec<PlannedOp>,
    pub asserts: Vec<Assertion>,
    pub assert_head: Option<ChangesetId>,
    /// Shared inputs applied to all content changes (`record --input`).
    pub inputs: Vec<InputRef>,
    pub skip_policy: bool,
}

impl CommitRequest {
    pub fn new(ref_name: &str, actor: &str) -> Self {
        CommitRequest {
            ref_name: ref_name.to_string(),
            base: None,
            actor: actor.to_string(),
            message: None,
            ops: vec![],
            asserts: vec![],
            assert_head: None,
            inputs: vec![],
            skip_policy: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CommitOutcome {
    pub changeset: ChangesetId,
    pub seq: i64,
    pub head_before: Option<ChangesetId>,
    pub actor_id: ActorId,
    pub changes: Vec<Change>,
    pub reports: Vec<Value>,
}

/// A failed precondition, before enrichment.
#[derive(Clone, Debug)]
struct RawConflict {
    kind: &'static str,
    op_index: usize,
    concept: Option<ConceptId>,
    section: Option<SectionId>,
    path: Option<String>,
    base_version: Option<ChangeId>,
    theirs_version: Option<ChangeId>,
    detail: Option<String>,
}

struct PreparedOp {
    planned: PlannedOp,
    payload: Option<PreparedPayload>,
    note: Option<PreparedPayload>,
    plaintext: Option<Vec<u8>>,
}

type NewActorRegistration = Option<(ActorRow, PreparedPayload)>;

/// Resolve the actor string to an existing actor, or plan its registration.
fn resolve_actor(
    repo: &Repository,
    name: &str,
) -> Result<(ActorId, ActorKind, NewActorRegistration)> {
    let kind = ActorKind::classify(name).ok_or_else(|| {
        WispError::usage(format!(
            "actor {name:?} must be <producer>/<version>, human:<id>, or process:<id>"
        ))
    })?;
    if let Some(row) = repo.find_actor(name)? {
        return Ok((row.id, row.kind, None));
    }
    let id = ActorId::new();
    let scope = ScopeId::new();
    repo.new_scope_key(scope)?;
    let name_payload = repo.prepare_payload(scope, PayloadKind::ActorName, name.as_bytes())?;
    let row = ActorRow {
        id,
        kind,
        scope,
        name: name_payload.reference,
        public_key: None,
        name_index: Some(repo.actor_index(name)?),
    };
    Ok((id, kind, Some((row, name_payload))))
}

fn validate_one(conn: &Connection, i: usize, p: &PlannedOp) -> Result<Option<RawConflict>> {
    let mk = |kind: &'static str,
              base: Option<ChangeId>,
              theirs: Option<ChangeId>,
              detail: Option<String>| RawConflict {
        kind,
        op_index: i,
        concept: p.target.concept(),
        section: p.target.section(),
        path: p.path.clone(),
        base_version: base,
        theirs_version: theirs,
        detail,
    };
    // Every operation: target scope is not erased (checked by the caller against the ledger).
    Ok(match &p.precondition {
        Precondition::None | Precondition::Any => None,
        Precondition::SectionVersion { section, version } => match Hot::section(conn, *section)? {
            None => Some(mk("section_removed", Some(*version), None, None)),
            Some(s) if s.version != *version => Some(mk(
                "section_conflict",
                Some(*version),
                Some(s.version),
                None,
            )),
            _ => None,
        },
        Precondition::ConceptLive(c) => {
            if Hot::concept(conn, *c)?.is_none() {
                let retracted = Hot::retracted(conn, *c)?.is_some();
                Some(mk(
                    "lifecycle_conflict",
                    None,
                    None,
                    Some(if retracted {
                        "concept is retracted".into()
                    } else {
                        "concept does not exist".into()
                    }),
                ))
            } else {
                None
            }
        }
        Precondition::ConceptRetracted(c) => {
            if Hot::retracted(conn, *c)?.is_none() {
                Some(mk(
                    "lifecycle_conflict",
                    None,
                    None,
                    Some("concept is not retracted".into()),
                ))
            } else {
                None
            }
        }
        Precondition::EntryVersion {
            concept,
            key,
            version,
        } => {
            let cur = Hot::frontmatter_entry(conn, *concept, key)?.map(|e| e.version);
            if cur != *version {
                Some(mk("frontmatter_conflict", *version, cur, Some(key.clone())))
            } else {
                None
            }
        }
        Precondition::SourceVersion {
            concept,
            id,
            version,
        } => {
            let cur = Hot::sources_of(conn, *concept)?
                .into_iter()
                .find(|s| &s.source_id == id)
                .map(|s| s.version);
            if cur != *version {
                Some(mk("source_conflict", *version, cur, Some(id.clone())))
            } else {
                None
            }
        }
        Precondition::PathFree(path) => {
            if Hot::concept_by_path(conn, path)?.is_some()
                || Hot::asset_by_path(conn, path)?.is_some()
            {
                Some(mk("path_conflict", None, None, Some(path.clone())))
            } else {
                None
            }
        }
        Precondition::SectionOrder { concept, order } => {
            let cur: Vec<SectionId> = Hot::sections_of(conn, *concept)?
                .into_iter()
                .map(|s| s.section)
                .collect();
            if &cur != order {
                Some(mk(
                    "section_conflict",
                    None,
                    None,
                    Some("section order changed".into()),
                ))
            } else {
                None
            }
        }
        Precondition::ContentVersion { concept, version } => {
            let cur = Hot::concept(conn, *concept)?.and_then(|c| c.content_version);
            if cur != Some(*version) {
                Some(mk(
                    "section_conflict",
                    Some(*version),
                    cur,
                    Some("content version changed".into()),
                ))
            } else {
                None
            }
        }
        Precondition::AssetVersion { asset, version } => match Hot::asset(conn, *asset)? {
            None => Some(mk(
                "section_removed",
                Some(*version),
                None,
                Some("asset removed".into()),
            )),
            Some(a) if a.version != *version => Some(mk(
                "section_conflict",
                Some(*version),
                Some(a.version),
                Some("asset changed".into()),
            )),
            _ => None,
        },
    })
}

/// Run an optimistic commit. Retries exit 7 up to `commit.retries` times, reusing prepared ciphertext.
pub fn commit(repo: &mut Repository, req: CommitRequest) -> Result<CommitOutcome> {
    if req.ops.is_empty() {
        return Err(WispError::usage("nothing to commit"));
    }
    // 1. Actor.
    let (actor_id, actor_kind, new_actor) = resolve_actor(repo, &req.actor)?;

    // 2. Policy.
    if !req.skip_policy {
        for p in &req.ops {
            let subj = PolicySubject {
                op: p.op,
                concept_type: p.policy.concept_type.as_deref(),
                section_heading: p.policy.section_heading.as_deref(),
                key: p.policy.key.as_deref(),
                unconditional: p.precondition == Precondition::Any,
            };
            if let Some(rule) =
                check_policy(&repo.manifest.policies, &subj, Some(&req.actor), actor_kind)
            {
                return Err(
                    WispError::refused(format!("refused by policy {rule}")).with_details(json!({
                        "rule": rule, "op": p.op.as_str(), "path": p.path, "actor": req.actor
                    })),
                );
            }
        }
    }

    // 3. Prepare, unlocked: keys for new scopes, encrypted payloads, notes, message.
    let mut prepared: Vec<PreparedOp> = Vec::with_capacity(req.ops.len());
    for p in &req.ops {
        if let Some(kind) = &p.new_scope {
            let _ = kind;
            repo.new_scope_key(p.scope)?;
        }
        let payload = match &p.payload {
            Some((kind, bytes)) => Some(repo.prepare_payload(p.scope, *kind, bytes)?),
            None => None,
        };
        let note = match &p.note {
            Some(n) => Some(repo.prepare_payload(p.scope, PayloadKind::Note, n.as_bytes())?),
            None => None,
        };
        prepared.push(PreparedOp {
            planned: p.clone(),
            payload,
            note,
            plaintext: p.payload.as_ref().map(|(_, b)| b.clone()),
        });
    }
    let message = match &req.message {
        Some(m) if !m.is_empty() => {
            Some(repo.prepare_payload(repo.repo_scope(), PayloadKind::Message, m.as_bytes())?)
        }
        _ => None,
    };

    let retries = repo.manifest.commit.retries.max(1);
    let mut attempt = 0;
    loop {
        attempt += 1;
        match try_commit(repo, &req, actor_id, &new_actor, &prepared, &message) {
            Ok(out) => {
                repo.ledger.flush_staged()?;
                return Ok(out);
            }
            Err(e) if e.is_retryable() && attempt < retries => {
                let jitter = 20 + (rand::random::<u64>() % 80) * attempt as u64;
                std::thread::sleep(std::time::Duration::from_millis(jitter));
                continue;
            }
            Err(e) => return Err(e),
        }
    }
}

fn try_commit(
    repo: &mut Repository,
    req: &CommitRequest,
    actor_id: ActorId,
    new_actor: &Option<(ActorRow, PreparedPayload)>,
    prepared: &[PreparedOp],
    message: &Option<PreparedPayload>,
) -> Result<CommitOutcome> {
    // Make sure the hot tier exists and is current before taking the ledger lock.
    repo.hot(&req.ref_name)?;
    let mut hot = repo.hots.remove(&req.ref_name).expect("hot opened");
    let res = try_commit_inner(repo, &mut hot, req, actor_id, new_actor, prepared, message);
    repo.hots.insert(req.ref_name.clone(), hot);
    res
}

fn try_commit_inner(
    repo: &mut Repository,
    hot: &mut Hot,
    req: &CommitRequest,
    actor_id: ActorId,
    new_actor: &Option<(ActorRow, PreparedPayload)>,
    prepared: &[PreparedOp],
    message: &Option<PreparedPayload>,
) -> Result<CommitOutcome> {
    let payloads_dir = repo.ledger.payloads_dir();
    let repo_id = repo.repo_id();
    let repo_scope = repo.repo_scope();
    let profile_id = repo.profile.id().to_string();
    let keystore = &repo.keystore;
    let now = now_rfc3339();

    // 2. Begin.
    let tx = repo.ledger.begin_immediate()?;
    // 3. Read head.
    let head = Ledger::head(&tx, &req.ref_name)?;
    if let Some(ah) = req.assert_head {
        if head != Some(ah) {
            drop(tx);
            return Err(WispError::conflict(
                "ref head moved",
                json!([{ "id": format!("cf_{}", ChangeId::new()), "type": "assertion_failed", "ref": req.ref_name, "expected": ah.to_string(), "head": head.map(|h| h.to_string()) }]),
            ));
        }
    }
    // The hot tier must reflect the head we just read.
    let meta = Hot::meta(hot.conn())?;
    let hot_current = match (&meta, head) {
        (Some(m), Some(h)) => m.head == h,
        (Some(m), None) => m.head == ChangesetId::from_bytes([0u8; 16]),
        (None, None) => false,
        (None, Some(_)) => false,
    };
    if !hot_current {
        drop(tx);
        crate::apply::rebuild_hot(repo, &req.ref_name)?;
        if let Some(fresh) = repo.hots.remove(&req.ref_name) {
            *hot = fresh;
        }
        return Err(WispError::timeout("hot tier was stale; rebuilt"));
    }

    // 4. Validate.
    let mut conflicts: Vec<RawConflict> = Vec::new();
    for (i, p) in prepared.iter().enumerate() {
        if let Some(s) = Ledger::scope(&tx, p.planned.scope)? {
            if s.erased {
                conflicts.push(RawConflict {
                    kind: "scope_erased",
                    op_index: i,
                    concept: p.planned.target.concept(),
                    section: p.planned.target.section(),
                    path: p.planned.path.clone(),
                    base_version: None,
                    theirs_version: None,
                    detail: None,
                });
                continue;
            }
        }
        if let Some(c) = validate_one(hot.conn(), i, &p.planned)? {
            conflicts.push(c);
        }
    }
    for a in &req.asserts {
        match a {
            Assertion::Section {
                section,
                version,
                path,
            } => {
                let cur = Hot::section(hot.conn(), *section)?.map(|s| s.version);
                if cur != Some(*version) {
                    conflicts.push(RawConflict {
                        kind: "assertion_failed",
                        op_index: usize::MAX,
                        concept: None,
                        section: Some(*section),
                        path: Some(path.clone()),
                        base_version: Some(*version),
                        theirs_version: cur,
                        detail: None,
                    });
                }
            }
            Assertion::Concept {
                concept,
                version,
                path,
            } => {
                let cur = Hot::concept(hot.conn(), *concept)?.map(|c| c.version);
                if cur != Some(*version) {
                    conflicts.push(RawConflict {
                        kind: "assertion_failed",
                        op_index: usize::MAX,
                        concept: Some(*concept),
                        section: None,
                        path: Some(path.clone()),
                        base_version: Some(*version),
                        theirs_version: cur,
                        detail: None,
                    });
                }
            }
        }
    }
    if !conflicts.is_empty() {
        // 6. Reject: roll back, then enrich conflict objects with content (§11.2).
        let reader = Reader {
            conn: &tx,
            payloads_dir: payloads_dir.clone(),
            keystore,
            repo_id,
        };
        let objs = enrich_conflicts(&reader, hot.conn(), &conflicts, prepared);
        drop(tx);
        return Err(WispError::conflict(
            "precondition failed",
            Value::Array(objs),
        ));
    }

    // 5. Commit.
    let cs_id = ChangesetId::new();
    let mut changes: Vec<Change> = Vec::with_capacity(prepared.len() + 1);
    let mut ordinal = 0u32;
    if let Some((row, name_payload)) = new_actor {
        // register_actor is appended automatically the first time an actor commits (§8).
        changes.push(Change {
            id: ChangeId::new(),
            changeset: cs_id,
            ordinal,
            op: Op::RegisterActor,
            target: Target::Actor { actor: row.id },
            scope: row.scope,
            payload: Some(name_payload.reference),
            note: None,
            inputs: vec![],
            valid: None,
            lineage: None,
            origin: None,
            reason: None,
        });
        ordinal += 1;
    }
    for p in prepared {
        let mut inputs = p.planned.inputs.clone();
        if p.planned.op.is_content_change() {
            for i in &req.inputs {
                if !inputs.contains(i) {
                    inputs.push(i.clone());
                }
            }
        }
        changes.push(Change {
            id: ChangeId::new(),
            changeset: cs_id,
            ordinal,
            op: p.planned.op,
            target: p.planned.target.clone(),
            scope: p.planned.scope,
            payload: p.payload.as_ref().map(|x| x.reference),
            note: p.note.as_ref().map(|x| x.reference),
            inputs,
            valid: None,
            lineage: p.planned.lineage.clone(),
            origin: p.planned.origin.clone(),
            reason: p.planned.reason,
        });
        ordinal += 1;
    }
    let parents: Vec<ChangesetId> = head.into_iter().collect();
    let mut parent_digests = Vec::new();
    for p in &parents {
        let row = Ledger::get_changeset(&tx, *p)?
            .ok_or_else(|| WispError::integrity("parent changeset missing"))?;
        parent_digests.push(row.changeset.digest);
    }
    let based_on = match (req.base, head) {
        (Some(b), Some(h)) if b != h => Some(b),
        (Some(b), None) => Some(b),
        _ => None,
    };
    let message_ref = message.as_ref().map(|m| m.reference);
    let digest = canonical::changeset_digest(
        cs_id,
        actor_id,
        &now,
        &parent_digests,
        based_on,
        message_ref.as_ref(),
        &changes,
    );
    let reader = Reader {
        conn: &tx,
        payloads_dir: payloads_dir.clone(),
        keystore,
        repo_id,
    };
    let sig = crate::signing::sign_digest(&repo.wisp_dir, &reader, actor_id, &digest)?;
    let cs = Changeset {
        id: cs_id,
        parents,
        based_on,
        actor_id,
        at: now.clone(),
        message: message_ref,
        digest,
        sig,
    };

    // Scopes, actors, payloads.
    if let Some((row, name_payload)) = new_actor {
        Ledger::insert_scope(&tx, row.scope, &ScopeKind::Actor { actor: row.id })?;
        Ledger::insert_actor(&tx, row)?;
        stage(&tx, name_payload)?;
    }
    for p in prepared {
        if let Some(kind) = &p.planned.new_scope {
            Ledger::insert_scope(&tx, p.planned.scope, kind)?;
        }
        if let Some(pl) = &p.payload {
            stage(&tx, pl)?;
        }
        if let Some(n) = &p.note {
            stage(&tx, n)?;
        }
    }
    if let Some(m) = message {
        stage(&tx, m)?;
    }
    let seq = Ledger::insert_changeset(&tx, &cs, &changes)?;
    Ledger::set_head(&tx, &req.ref_name, cs_id)?;
    // Mutable bookkeeping derived from this changeset.
    for (p, c) in prepared
        .iter()
        .zip(changes.iter().skip(if new_actor.is_some() { 1 } else { 0 }))
    {
        match (&p.planned.op, &p.planned.target) {
            (Op::MoveSection, Target::Section { concept, .. }) => {
                Ledger::set_scope_concept(&tx, p.planned.scope, *concept)?
            }
            (Op::GroupAdd, Target::Group { group })
            | (Op::GroupRemove, Target::Group { group }) => {
                if let Some(pt) = &p.plaintext {
                    let m = crate::payloads::decode_group_member(pt)?;
                    let member = ulid_bytes(&m.member);
                    if p.planned.op == Op::GroupAdd {
                        Ledger::group_add_member(&tx, *group, &member, &m.kind)?;
                    } else {
                        Ledger::group_remove_member(&tx, *group, &member)?;
                    }
                }
            }
            (Op::ActorKey, Target::Actor { actor }) => {
                if let Some(r) = &c.payload {
                    Ledger::set_actor_key(&tx, *actor, r)?;
                }
            }
            _ => {}
        }
    }

    // Hot tier, inside the same critical section, then commit both.
    {
        let reader = Reader {
            conn: &tx,
            payloads_dir: payloads_dir.clone(),
            keystore,
            repo_id,
        };
        let htx = hot.begin()?;
        let offset = if new_actor.is_some() { 1 } else { 0 };
        for (i, c) in changes.iter().enumerate() {
            if i < offset {
                continue;
            }
            let p = &prepared[i - offset];
            if c.op == Op::Restore {
                match &c.target {
                    Target::Concept { concept } => {
                        crate::apply::replay_concept(&reader, &htx, *concept)?
                    }
                    Target::Asset { asset } => crate::apply::replay_asset(&reader, &htx, *asset)?,
                    _ => {}
                }
            } else {
                crate::apply::apply_change(&htx, &cs, c, p.plaintext.as_deref())?;
            }
        }
        Hot::advance(&htx, cs_id, seq)?;
        // The hot tier commits first so no writer that acquires the ledger lock next can observe a
        // head whose changes are not yet in the tier. If the process dies between the two commits the
        // tier is ahead of the ledger; `Repository::hot` detects the unknown head and rebuilds it.
        htx.commit()
            .map_err(|e| WispError::internal(format!("hot commit: {e}")))?;
        tx.commit()
            .map_err(|e| WispError::internal(format!("ledger commit: {e}")))?;
    }
    let _ = (repo_scope, profile_id);
    Ok(CommitOutcome {
        changeset: cs_id,
        seq,
        head_before: head,
        actor_id,
        changes,
        reports: vec![],
    })
}

/// Group members are concept, section, or asset IDs (all ULIDs); store their raw bytes.
fn ulid_bytes(s: &str) -> Vec<u8> {
    s.parse::<SectionId>()
        .map(|x| x.bytes().to_vec())
        .unwrap_or_else(|_| s.as_bytes().to_vec())
}

fn stage(tx: &Connection, p: &PreparedPayload) -> Result<()> {
    Ledger::insert_payload_row(tx, p.reference.id, p.scope, p.kind, p.size)?;
    if let Some(ct) = &p.ciphertext {
        Ledger::stage_payload(tx, p.reference.id, ct)?;
    }
    Ok(())
}

fn enrich_conflicts(
    reader: &Reader<'_>,
    hot: &Connection,
    raw: &[RawConflict],
    prepared: &[PreparedOp],
) -> Vec<Value> {
    raw.iter()
        .map(|c| {
            let mut obj = json!({
                "id": format!("cf_{}", ChangeId::new()),
                "type": c.kind,
            });
            if let Some(cp) = c.concept {
                obj["concept"] = json!(cp.to_string());
            }
            if let Some(p) = &c.path {
                obj["path"] = json!(p);
            }
            if let Some(s) = c.section {
                obj["section"] = json!(s.to_string());
            }
            if let Some(d) = &c.detail {
                obj["detail"] = json!(d);
            }
            if c.kind == "scope_erased" {
                return obj;
            }
            // base
            if let Some(bv) = c.base_version {
                let mut base = json!({ "version": bv.to_string() });
                if let Ok(Some(ch)) = Ledger::get_change(reader.conn, bv) {
                    if let Some(pr) = &ch.payload {
                        if let Ok(Some(pt)) = reader.read_payload(ch.scope, pr) {
                            base["content"] = json!(content_of(ch.op, &pt));
                        }
                    }
                }
                obj["base"] = base;
            }
            // theirs
            if let Some(tv) = c.theirs_version {
                let mut theirs = json!({ "version": tv.to_string() });
                if let Ok(Some(ch)) = Ledger::get_change(reader.conn, tv) {
                    theirs["changeset"] = json!(ch.changeset.to_string());
                    if let Ok(Some(row)) = Ledger::get_changeset(reader.conn, ch.changeset) {
                        if let Ok(Some(a)) = Ledger::actor(reader.conn, row.changeset.actor_id) {
                            if let Ok(Some(n)) = reader.read_payload_string(a.scope, &a.name) {
                                theirs["actor"] = json!(n);
                            }
                        }
                    }
                }
                if let Some(s) = c.section {
                    if let Ok(Some(st)) = Hot::section(hot, s) {
                        theirs["content"] = json!(st.body);
                        theirs["heading"] = json!(st.heading);
                    }
                }
                obj["theirs"] = theirs;
            }
            // yours
            if c.op_index != usize::MAX {
                if let Some(p) = prepared.get(c.op_index) {
                    if let Some(pt) = &p.plaintext {
                        obj["yours"] = json!({ "content": content_of(p.planned.op, pt) });
                    }
                }
            }
            obj
        })
        .collect()
}

/// Record a two-parent merge changeset on `ref_name` and advance its head (§11.5).
pub fn record_merge(
    repo: &mut Repository,
    ref_name: &str,
    into_head: ChangesetId,
    source_head: ChangesetId,
) -> Result<ChangesetId> {
    let (actor_id, _kind, new_actor) = resolve_actor(repo, "process:wisp-merge")?;
    let now = now_rfc3339();
    let cs_id = ChangesetId::new();
    let mut changes: Vec<Change> = Vec::new();
    if let Some((row, name_payload)) = &new_actor {
        changes.push(Change {
            id: ChangeId::new(),
            changeset: cs_id,
            ordinal: 0,
            op: Op::RegisterActor,
            target: Target::Actor { actor: row.id },
            scope: row.scope,
            payload: Some(name_payload.reference),
            note: None,
            inputs: vec![],
            valid: None,
            lineage: None,
            origin: None,
            reason: None,
        });
    }
    let message = repo.prepare_payload(repo.repo_scope(), PayloadKind::Message, b"merge")?;
    let parents = vec![into_head, source_head];
    let mut parent_digests = Vec::new();
    let payloads_dir = repo.ledger.payloads_dir();
    let repo_id = repo.repo_id();
    let keystore = &repo.keystore;
    let wisp_dir = repo.wisp_dir.clone();
    let tx = repo.ledger.begin_immediate()?;
    for p in &parents {
        let row = Ledger::get_changeset(&tx, *p)?
            .ok_or_else(|| WispError::integrity("merge parent missing"))?;
        parent_digests.push(row.changeset.digest);
    }
    let digest = canonical::changeset_digest(
        cs_id,
        actor_id,
        &now,
        &parent_digests,
        Some(into_head),
        Some(&message.reference),
        &changes,
    );
    let reader = Reader {
        conn: &tx,
        payloads_dir,
        keystore,
        repo_id,
    };
    let sig = crate::signing::sign_digest(&wisp_dir, &reader, actor_id, &digest)?;
    let cs = Changeset {
        id: cs_id,
        parents,
        based_on: Some(into_head),
        actor_id,
        at: now,
        message: Some(message.reference),
        digest,
        sig,
    };
    if let Some((row, name_payload)) = &new_actor {
        Ledger::insert_scope(&tx, row.scope, &ScopeKind::Actor { actor: row.id })?;
        Ledger::insert_actor(&tx, row)?;
        stage(&tx, name_payload)?;
    }
    stage(&tx, &message)?;
    let _seq = Ledger::insert_changeset(&tx, &cs, &changes)?;
    Ledger::set_head(&tx, ref_name, cs_id)?;
    tx.commit()
        .map_err(|e| WispError::internal(e.to_string()))?;
    repo.ledger.flush_staged()?;
    Ok(cs_id)
}

fn content_of(op: Op, pt: &[u8]) -> Value {
    match op {
        Op::AddSection | Op::UpdateSection | Op::Split | Op::MoveSection => {
            match crate::payloads::SectionBody::decode(pt) {
                Ok(sb) => json!(sb.body),
                Err(_) => json!(String::from_utf8_lossy(pt)),
            }
        }
        Op::SetKey | Op::UnsetKey => match crate::payloads::decode_frontmatter(pt) {
            Ok(f) => json!(f.raw),
            Err(_) => json!(String::from_utf8_lossy(pt)),
        },
        _ => json!(String::from_utf8_lossy(pt)),
    }
}
