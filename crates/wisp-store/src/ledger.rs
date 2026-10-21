//! The cold tier (§6.1): an append-only metadata ledger plus encrypted payload files.
//!
//! Ledger log tables are insert-only; triggers reject UPDATE and DELETE (invariant I1).
//! Mutable bookkeeping (refs, worktrees, actors, payload presence, scopes, staged payloads,
//! cursors, pending erasures, groups) lives in separate tables.

use crate::{blob16, sql_err, SqlResult};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::{Path, PathBuf};
use wisp_model::*;

type ErasureMarkerRow = (SectionId, ConceptId, Option<SectionId>, Reason);

pub const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS changesets (
  id        BLOB PRIMARY KEY,
  seq       INTEGER UNIQUE NOT NULL,
  actor_id  BLOB NOT NULL,
  at        TEXT NOT NULL,
  message   BLOB,
  based_on  BLOB,
  digest    BLOB NOT NULL,
  sig       BLOB
);
CREATE TABLE IF NOT EXISTS changeset_parents (
  child  BLOB NOT NULL REFERENCES changesets(id),
  parent BLOB NOT NULL REFERENCES changesets(id),
  idx    INTEGER NOT NULL,
  PRIMARY KEY (child, idx)
);
CREATE TABLE IF NOT EXISTS changes (
  id        BLOB PRIMARY KEY,
  changeset BLOB NOT NULL REFERENCES changesets(id),
  ordinal   INTEGER NOT NULL,
  op        TEXT NOT NULL,
  target    BLOB NOT NULL,
  concept   BLOB,
  section   BLOB,
  scope     BLOB NOT NULL,
  payload   BLOB,
  note      BLOB,
  inputs    BLOB,
  valid     BLOB,
  lineage   BLOB,
  origin    BLOB,
  reason    TEXT,
  UNIQUE (changeset, ordinal)
);
CREATE INDEX IF NOT EXISTS changes_by_concept ON changes(concept, section);
CREATE INDEX IF NOT EXISTS changes_by_op ON changes(op);
CREATE TABLE IF NOT EXISTS tombstones (
  erase_change BLOB PRIMARY KEY REFERENCES changes(id),
  target       BLOB NOT NULL,
  scopes       BLOB NOT NULL,
  reason       TEXT NOT NULL,
  flagged      BLOB
);
CREATE TABLE IF NOT EXISTS exports (
  id BLOB PRIMARY KEY, changeset BLOB NOT NULL, kind TEXT NOT NULL, at TEXT NOT NULL
);

-- Mutable bookkeeping
CREATE TABLE IF NOT EXISTS refs (name TEXT PRIMARY KEY, head BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS worktrees (
  name TEXT PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  ref  TEXT NOT NULL UNIQUE REFERENCES refs(name),
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS actors (
  id         BLOB PRIMARY KEY,
  kind       TEXT NOT NULL,
  scope      BLOB NOT NULL,
  name       BLOB NOT NULL,
  public_key BLOB,
  name_index BLOB UNIQUE
);
CREATE TABLE IF NOT EXISTS payloads (
  id BLOB PRIMARY KEY, scope BLOB NOT NULL, kind TEXT NOT NULL,
  size INTEGER NOT NULL, present INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS payloads_by_scope ON payloads(scope);
CREATE TABLE IF NOT EXISTS scopes (
  id BLOB PRIMARY KEY,
  kind TEXT NOT NULL,
  concept BLOB,
  section BLOB,
  asset BLOB,
  erased INTEGER NOT NULL DEFAULT 0,
  shredded INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS scopes_by_concept ON scopes(concept);
CREATE TABLE IF NOT EXISTS staged_payloads (
  id         BLOB PRIMARY KEY,
  rel_path   TEXT NOT NULL,
  ciphertext BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS cursors (subscriber TEXT PRIMARY KEY, seq INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS pending_erasures (
  erase_change BLOB NOT NULL, subscriber TEXT NOT NULL,
  PRIMARY KEY (erase_change, subscriber)
);
CREATE TABLE IF NOT EXISTS groups (
  id    BLOB PRIMARY KEY,
  label BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS group_members (
  group_id BLOB NOT NULL REFERENCES groups(id),
  member   BLOB NOT NULL,
  kind     TEXT NOT NULL,
  PRIMARY KEY (group_id, member)
);
-- Where an erased section sat, so views can render its marker after a rebuild (bookkeeping).
CREATE TABLE IF NOT EXISTS erasure_markers (
  section BLOB PRIMARY KEY,
  concept BLOB NOT NULL,
  after_section BLOB,
  reason TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS ledger_flags (
  name TEXT PRIMARY KEY,
  value INTEGER NOT NULL
);
-- Replay skips chosen at merge resolution (durable; cache file is optional).
CREATE TABLE IF NOT EXISTS merge_replay_skips (
  ref_name TEXT NOT NULL,
  merge_changeset BLOB NOT NULL,
  change_id BLOB NOT NULL,
  PRIMARY KEY (ref_name, merge_changeset, change_id)
);
CREATE INDEX IF NOT EXISTS merge_replay_skips_by_head ON merge_replay_skips(ref_name, merge_changeset);

-- Insert-only log tables (I1)
CREATE TRIGGER IF NOT EXISTS changesets_no_update BEFORE UPDATE ON changesets BEGIN SELECT RAISE(ABORT, 'ledger is append-only'); END;
CREATE TRIGGER IF NOT EXISTS changesets_no_delete BEFORE DELETE ON changesets BEGIN SELECT RAISE(ABORT, 'ledger is append-only'); END;
CREATE TRIGGER IF NOT EXISTS parents_no_update BEFORE UPDATE ON changeset_parents BEGIN SELECT RAISE(ABORT, 'ledger is append-only'); END;
CREATE TRIGGER IF NOT EXISTS parents_no_delete BEFORE DELETE ON changeset_parents BEGIN SELECT RAISE(ABORT, 'ledger is append-only'); END;
CREATE TRIGGER IF NOT EXISTS changes_no_update BEFORE UPDATE ON changes BEGIN SELECT RAISE(ABORT, 'ledger is append-only'); END;
CREATE TRIGGER IF NOT EXISTS changes_no_delete BEFORE DELETE ON changes BEGIN SELECT RAISE(ABORT, 'ledger is append-only'); END;
CREATE TRIGGER IF NOT EXISTS tombstones_no_update BEFORE UPDATE ON tombstones BEGIN SELECT RAISE(ABORT, 'ledger is append-only'); END;
CREATE TRIGGER IF NOT EXISTS tombstones_no_delete BEFORE DELETE ON tombstones BEGIN SELECT RAISE(ABORT, 'ledger is append-only'); END;
"#;

/// A ledger row for a changeset plus its local sequence number.
#[derive(Clone, Debug)]
pub struct ChangesetRow {
    pub seq: i64,
    pub changeset: Changeset,
}

/// A tombstone row (§6.1).
#[derive(Clone, Debug)]
pub struct Tombstone {
    pub erase_change: ChangeId,
    pub target: Target,
    pub scopes: Vec<ScopeId>,
    pub reason: Reason,
    pub flagged: Vec<ConceptId>,
}

/// An actor row (materialized from register_actor / actor_key).
#[derive(Clone, Debug)]
pub struct ActorRow {
    pub id: ActorId,
    pub kind: ActorKind,
    pub scope: ScopeId,
    pub name: PayloadRef,
    pub public_key: Option<PayloadRef>,
    pub name_index: Option<[u8; 32]>,
}

/// A scope row.
#[derive(Clone, Debug)]
pub struct ScopeRow {
    pub id: ScopeId,
    pub kind: ScopeKind,
    pub erased: bool,
    pub shredded: bool,
}

pub struct Ledger {
    conn: Connection,
    wisp_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct WorktreeRow {
    pub name: String,
    pub path: String,
    pub ref_name: String,
    pub created_at: String,
}

impl Ledger {
    pub fn open(wisp_dir: &Path, busy_timeout_ms: u64) -> Result<Self> {
        let path = wisp_dir.join("ledger.db");
        let conn = Connection::open(&path).map_err(sql_err)?;
        conn.busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
            .map_err(sql_err)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA secure_delete = ON; PRAGMA auto_vacuum = INCREMENTAL; PRAGMA synchronous = NORMAL;")
            .map_err(sql_err)?;
        conn.execute_batch(SCHEMA).map_err(sql_err)?;
        Ok(Ledger {
            conn,
            wisp_dir: wisp_dir.to_path_buf(),
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
    pub fn wisp_dir(&self) -> &Path {
        &self.wisp_dir
    }
    pub fn payloads_dir(&self) -> PathBuf {
        self.wisp_dir.join("payloads")
    }

    /// `BEGIN IMMEDIATE`; the busy timeout turns contention into exit 7 (retryable).
    pub fn begin_immediate(&mut self) -> Result<Transaction<'_>> {
        self.conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::DatabaseBusy
                        || f.code == rusqlite::ErrorCode::DatabaseLocked =>
                {
                    WispError::timeout("write queue timeout")
                }
                e => sql_err(e),
            })
    }

    // ---- refs ----

    pub fn head(conn: &Connection, name: &str) -> Result<Option<ChangesetId>> {
        conn.query_row(
            "SELECT head FROM refs WHERE name = ?1",
            params![name],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(sql_err)
        .map(|o| o.and_then(|b| ChangesetId::from_slice(&b)))
    }
    pub fn set_head(conn: &Connection, name: &str, head: ChangesetId) -> Result<()> {
        conn.execute(
            "INSERT INTO refs (name, head) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET head = excluded.head",
            params![name, head.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn refs(conn: &Connection) -> Result<Vec<(String, ChangesetId)>> {
        let mut st = conn
            .prepare("SELECT name, head FROM refs ORDER BY name")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|(n, b)| ChangesetId::from_slice(&b).map(|c| (n, c)))
            .collect();
        Ok(rows)
    }
    pub fn delete_ref(conn: &Connection, name: &str) -> Result<()> {
        conn.execute("DELETE FROM refs WHERE name = ?1", params![name])
            .map_err(sql_err)?;
        Ok(())
    }

    // ---- worktrees ----

    pub fn insert_worktree(
        conn: &Connection,
        name: &str,
        path: &str,
        ref_name: &str,
        created_at: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO worktrees (name, path, ref, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, path, ref_name, created_at],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    pub fn delete_worktree(conn: &Connection, name: &str) -> Result<bool> {
        let n = conn
            .execute("DELETE FROM worktrees WHERE name = ?1", params![name])
            .map_err(sql_err)?;
        Ok(n > 0)
    }

    pub fn get_worktree(conn: &Connection, name: &str) -> Result<Option<WorktreeRow>> {
        conn.query_row(
            "SELECT name, path, ref, created_at FROM worktrees WHERE name = ?1",
            params![name],
            |r| {
                Ok(WorktreeRow {
                    name: r.get(0)?,
                    path: r.get(1)?,
                    ref_name: r.get(2)?,
                    created_at: r.get(3)?,
                })
            },
        )
        .optional()
        .map_err(sql_err)
    }

    pub fn worktree_by_ref(conn: &Connection, ref_name: &str) -> Result<Option<WorktreeRow>> {
        conn.query_row(
            "SELECT name, path, ref, created_at FROM worktrees WHERE ref = ?1",
            params![ref_name],
            |r| {
                Ok(WorktreeRow {
                    name: r.get(0)?,
                    path: r.get(1)?,
                    ref_name: r.get(2)?,
                    created_at: r.get(3)?,
                })
            },
        )
        .optional()
        .map_err(sql_err)
    }

    pub fn list_worktrees(conn: &Connection) -> Result<Vec<WorktreeRow>> {
        let mut st = conn
            .prepare("SELECT name, path, ref, created_at FROM worktrees ORDER BY name")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| {
                Ok(WorktreeRow {
                    name: r.get(0)?,
                    path: r.get(1)?,
                    ref_name: r.get(2)?,
                    created_at: r.get(3)?,
                })
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    // ---- changesets ----

    pub fn max_seq(conn: &Connection) -> Result<i64> {
        conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM changesets", [], |r| {
            r.get(0)
        })
        .map_err(sql_err)
    }

    /// Append a changeset and its changes. Returns the assigned seq.
    pub fn insert_changeset(conn: &Connection, cs: &Changeset, changes: &[Change]) -> Result<i64> {
        let seq = Self::max_seq(conn)? + 1;
        conn.execute(
            "INSERT INTO changesets (id, seq, actor_id, at, message, based_on, digest, sig) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                cs.id.bytes().to_vec(),
                seq,
                cs.actor_id.bytes().to_vec(),
                cs.at,
                cs.message.as_ref().map(|p| p.to_blob()),
                cs.based_on.map(|b| b.bytes().to_vec()),
                cs.digest.to_vec(),
                cs.sig.clone(),
            ],
        )
        .map_err(sql_err)?;
        for (i, p) in cs.parents.iter().enumerate() {
            conn.execute(
                "INSERT INTO changeset_parents (child, parent, idx) VALUES (?1, ?2, ?3)",
                params![cs.id.bytes().to_vec(), p.bytes().to_vec(), i as i64],
            )
            .map_err(sql_err)?;
        }
        for c in changes {
            conn.execute(
                "INSERT INTO changes (id, changeset, ordinal, op, target, concept, section, scope, payload, note, inputs, valid, lineage, origin, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    c.id.bytes().to_vec(),
                    c.changeset.bytes().to_vec(),
                    c.ordinal as i64,
                    c.op.as_str(),
                    serde_json::to_vec(&c.target)?,
                    c.target.concept().map(|x| x.bytes().to_vec()),
                    c.target.section().map(|x| x.bytes().to_vec()),
                    c.scope.bytes().to_vec(),
                    c.payload.as_ref().map(|p| p.to_blob()),
                    c.note.as_ref().map(|p| p.to_blob()),
                    if c.inputs.is_empty() { None } else { Some(serde_json::to_vec(&c.inputs)?) },
                    c.valid.as_ref().map(serde_json::to_vec).transpose()?,
                    c.lineage.as_ref().map(serde_json::to_vec).transpose()?,
                    c.origin.as_ref().map(serde_json::to_vec).transpose()?,
                    c.reason.map(|r| r.as_str()),
                ],
            )
            .map_err(sql_err)?;
        }
        Ok(seq)
    }

    fn row_to_changeset(r: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, Changeset)> {
        let id = blob16(r.get::<_, Vec<u8>>(0)?);
        let seq: i64 = r.get(1)?;
        let actor = blob16(r.get::<_, Vec<u8>>(2)?);
        let at: String = r.get(3)?;
        let message: Option<Vec<u8>> = r.get(4)?;
        let based_on: Option<Vec<u8>> = r.get(5)?;
        let digest: Vec<u8> = r.get(6)?;
        let sig: Option<Vec<u8>> = r.get(7)?;
        let mut d = [0u8; 32];
        if digest.len() == 32 {
            d.copy_from_slice(&digest);
        }
        Ok((
            seq,
            Changeset {
                id: ChangesetId::from_bytes(id),
                parents: vec![],
                based_on: based_on.and_then(|b| ChangesetId::from_slice(&b)),
                actor_id: ActorId::from_bytes(actor),
                at,
                message: message.and_then(|b| PayloadRef::from_blob(&b)),
                digest: d,
                sig,
            },
        ))
    }

    fn fill_parents(conn: &Connection, cs: &mut Changeset) -> Result<()> {
        let mut st = conn
            .prepare("SELECT parent FROM changeset_parents WHERE child = ?1 ORDER BY idx")
            .map_err(sql_err)?;
        cs.parents = st
            .query_map(params![cs.id.bytes().to_vec()], |r| r.get::<_, Vec<u8>>(0))
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|b| ChangesetId::from_slice(&b))
            .collect();
        Ok(())
    }

    pub fn get_changeset(conn: &Connection, id: ChangesetId) -> Result<Option<ChangesetRow>> {
        let row = conn
            .query_row(
                "SELECT id, seq, actor_id, at, message, based_on, digest, sig FROM changesets WHERE id = ?1",
                params![id.bytes().to_vec()],
                Self::row_to_changeset,
            )
            .optional()
            .map_err(sql_err)?;
        match row {
            None => Ok(None),
            Some((seq, mut cs)) => {
                Self::fill_parents(conn, &mut cs)?;
                Ok(Some(ChangesetRow { seq, changeset: cs }))
            }
        }
    }

    /// All changesets reachable from `head` (parents + `based_on`), oldest-first, each id once.
    pub fn changesets_closure(conn: &Connection, head: ChangesetId) -> Result<Vec<ChangesetRow>> {
        use std::collections::{HashSet, VecDeque};
        let mut seen = HashSet::new();
        let mut stack = VecDeque::from([head]);
        let mut rows = Vec::new();
        while let Some(id) = stack.pop_front() {
            if !seen.insert(id) {
                continue;
            }
            let Some(row) = Self::get_changeset(conn, id)? else {
                continue;
            };
            if let Some(b) = row.changeset.based_on {
                stack.push_back(b);
            }
            for p in &row.changeset.parents {
                stack.push_back(*p);
            }
            rows.push(row);
        }
        rows.sort_by_key(|r| r.seq);
        let mut out = Vec::new();
        let mut ids = HashSet::new();
        for r in rows {
            if ids.insert(r.changeset.id) {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// All changesets in seq order (oldest first).
    pub fn changesets(conn: &Connection) -> Result<Vec<ChangesetRow>> {
        let mut st = conn
            .prepare("SELECT id, seq, actor_id, at, message, based_on, digest, sig FROM changesets ORDER BY seq")
            .map_err(sql_err)?;
        let rows: Vec<(i64, Changeset)> = st
            .query_map([], Self::row_to_changeset)
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        let mut out = Vec::with_capacity(rows.len());
        for (seq, mut cs) in rows {
            Self::fill_parents(conn, &mut cs)?;
            out.push(ChangesetRow { seq, changeset: cs });
        }
        Ok(out)
    }

    fn row_to_change(r: &rusqlite::Row<'_>) -> rusqlite::Result<Change> {
        let target: Vec<u8> = r.get(4)?;
        let inputs: Option<Vec<u8>> = r.get(8)?;
        let valid: Option<Vec<u8>> = r.get(9)?;
        let lineage: Option<Vec<u8>> = r.get(10)?;
        let origin: Option<Vec<u8>> = r.get(11)?;
        let reason: Option<String> = r.get(12)?;
        Ok(Change {
            id: ChangeId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?)),
            changeset: ChangesetId::from_bytes(blob16(r.get::<_, Vec<u8>>(1)?)),
            ordinal: r.get::<_, i64>(2)? as u32,
            op: Op::parse(&r.get::<_, String>(3)?).unwrap_or(Op::Verify),
            target: serde_json::from_slice(&target).unwrap_or(Target::Erase { scopes: vec![] }),
            scope: ScopeId::from_bytes(blob16(r.get::<_, Vec<u8>>(5)?)),
            payload: r
                .get::<_, Option<Vec<u8>>>(6)?
                .and_then(|b| PayloadRef::from_blob(&b)),
            note: r
                .get::<_, Option<Vec<u8>>>(7)?
                .and_then(|b| PayloadRef::from_blob(&b)),
            inputs: inputs
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
            valid: valid.and_then(|b| serde_json::from_slice(&b).ok()),
            lineage: lineage.and_then(|b| serde_json::from_slice(&b).ok()),
            origin: origin.and_then(|b| serde_json::from_slice(&b).ok()),
            reason: reason.and_then(|s| Reason::parse(&s)),
        })
    }

    const CHANGE_COLS: &'static str =
        "c.id, c.changeset, c.ordinal, c.op, c.target, c.scope, c.payload, c.note, c.inputs, c.valid, c.lineage, c.origin, c.reason";

    pub fn changes_of(conn: &Connection, cs: ChangesetId) -> Result<Vec<Change>> {
        let sql = format!(
            "SELECT {} FROM changes c WHERE c.changeset = ?1 ORDER BY c.ordinal",
            Self::CHANGE_COLS
        );
        let mut st = conn.prepare(&sql).map_err(sql_err)?;
        let rows = st
            .query_map(params![cs.bytes().to_vec()], Self::row_to_change)
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn get_change(conn: &Connection, id: ChangeId) -> Result<Option<Change>> {
        let sql = format!(
            "SELECT {} FROM changes c WHERE c.id = ?1",
            Self::CHANGE_COLS
        );
        conn.query_row(&sql, params![id.bytes().to_vec()], Self::row_to_change)
            .optional()
            .map_err(sql_err)
    }

    /// The feed: changes ordered by (changesets.seq, ordinal) with seq > `since`.
    pub fn changes_since(conn: &Connection, since: i64) -> Result<Vec<(i64, Change)>> {
        let sql = format!(
            "SELECT {}, s.seq FROM changes c JOIN changesets s ON s.id = c.changeset WHERE s.seq > ?1 ORDER BY s.seq, c.ordinal",
            Self::CHANGE_COLS
        );
        let mut st = conn.prepare(&sql).map_err(sql_err)?;
        let rows = st
            .query_map(params![since], |r| {
                Ok((r.get::<_, i64>(13)?, Self::row_to_change(r)?))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Changes touching a concept (and optionally a section), oldest first.
    pub fn changes_for_concept(
        conn: &Connection,
        concept: ConceptId,
        section: Option<SectionId>,
    ) -> Result<Vec<(i64, Change)>> {
        let sql = format!(
            "SELECT {}, s.seq FROM changes c JOIN changesets s ON s.id = c.changeset WHERE c.concept = ?1 {} ORDER BY s.seq, c.ordinal",
            Self::CHANGE_COLS,
            if section.is_some() { "AND c.section = ?2" } else { "AND (?2 IS NULL OR 1)" }
        );
        let mut st = conn.prepare(&sql).map_err(sql_err)?;
        let rows = st
            .query_map(
                params![
                    concept.bytes().to_vec(),
                    section.map(|s| s.bytes().to_vec())
                ],
                |r| Ok((r.get::<_, i64>(13)?, Self::row_to_change(r)?)),
            )
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    // ---- payloads ----

    pub fn payload_rel_path(id: PayloadId) -> String {
        let h = hex::encode(id.bytes());
        format!("{}/{}", &h[..2], h)
    }

    pub fn insert_payload_row(
        conn: &Connection,
        id: PayloadId,
        scope: ScopeId,
        kind: PayloadKind,
        size: usize,
    ) -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO payloads (id, scope, kind, size, present) VALUES (?1, ?2, ?3, ?4, 1)",
            params![id.bytes().to_vec(), scope.bytes().to_vec(), kind.as_str(), size as i64],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    /// Find an existing payload in a scope by commitment (dedup within a scope only, §10.2).
    pub fn find_payload_by_commit(
        conn: &Connection,
        scope: ScopeId,
        commit: &Commit,
    ) -> Result<Option<PayloadId>> {
        // Commitments are stored only inside change rows (payload = id||commit); scan this scope's changes.
        let mut st = conn
            .prepare("SELECT payload FROM changes WHERE scope = ?1 AND payload IS NOT NULL")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![scope.bytes().to_vec()], |r| r.get::<_, Vec<u8>>(0))
            .map_err(sql_err)?
            .filter_map(|r| r.ok());
        for b in rows {
            if let Some(p) = PayloadRef::from_blob(&b) {
                if &p.commit == commit {
                    let present: Option<i64> = conn
                        .query_row(
                            "SELECT present FROM payloads WHERE id = ?1",
                            params![p.id.bytes().to_vec()],
                            |r| r.get(0),
                        )
                        .optional()
                        .map_err(sql_err)?;
                    if present == Some(1) {
                        return Ok(Some(p.id));
                    }
                }
            }
        }
        Ok(None)
    }

    pub fn stage_payload(conn: &Connection, id: PayloadId, ciphertext: &[u8]) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO staged_payloads (id, rel_path, ciphertext) VALUES (?1, ?2, ?3)",
            params![id.bytes().to_vec(), Self::payload_rel_path(id), ciphertext],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    /// Flush staged payloads to files through a temp file, fsync, and atomic rename (§10.2 step 3). Idempotent.
    pub fn flush_staged(&self) -> Result<usize> {
        let mut st = self
            .conn
            .prepare("SELECT id, rel_path, ciphertext FROM staged_payloads")
            .map_err(sql_err)?;
        let rows: Vec<(Vec<u8>, String, Vec<u8>)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        drop(st);
        let dir = self.payloads_dir();
        let mut n = 0;
        for (id, rel, ct) in rows {
            let final_path = dir.join(&rel);
            if let Some(parent) = final_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Another process may flush the same row concurrently: unique temp names, and a lost race is fine.
            let tmp = final_path.with_extension(format!("{}.tmp", ulid_suffix()));
            {
                use std::io::Write;
                let mut f = std::fs::File::create(&tmp)?;
                f.write_all(&ct)?;
                f.sync_all()?;
            }
            if let Err(e) = std::fs::rename(&tmp, &final_path) {
                let _ = std::fs::remove_file(&tmp);
                if !final_path.exists() {
                    return Err(e.into());
                }
            }
            self.conn
                .execute("DELETE FROM staged_payloads WHERE id = ?1", params![id])
                .map_err(sql_err)?;
            n += 1;
        }
        Ok(n)
    }

    /// Read a payload's ciphertext: staged rows first, then the file (§10.2 step 4).
    pub fn read_payload(&self, id: PayloadId) -> Result<Option<Vec<u8>>> {
        Self::read_payload_at(&self.conn, &self.payloads_dir(), id)
    }

    /// Static form of `read_payload`, usable inside a transaction.
    pub fn read_payload_at(
        conn: &Connection,
        payloads_dir: &Path,
        id: PayloadId,
    ) -> Result<Option<Vec<u8>>> {
        let staged: Option<Vec<u8>> = conn
            .query_row(
                "SELECT ciphertext FROM staged_payloads WHERE id = ?1",
                params![id.bytes().to_vec()],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)?;
        if staged.is_some() {
            return Ok(staged);
        }
        let p = payloads_dir.join(Self::payload_rel_path(id));
        match std::fs::read(&p) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn payload_info(
        conn: &Connection,
        id: PayloadId,
    ) -> Result<Option<(ScopeId, PayloadKind, usize)>> {
        conn.query_row(
            "SELECT scope, kind, size FROM payloads WHERE id = ?1",
            params![id.bytes().to_vec()],
            |r| {
                let scope = ScopeId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?));
                let kind: String = r.get(1)?;
                let size: i64 = r.get(2)?;
                Ok((scope, payload_kind_from_str(&kind), size as usize))
            },
        )
        .optional()
        .map_err(sql_err)
    }

    pub fn payload_present(conn: &Connection, id: PayloadId) -> Result<bool> {
        let p: Option<i64> = conn
            .query_row(
                "SELECT present FROM payloads WHERE id = ?1",
                params![id.bytes().to_vec()],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)?;
        Ok(p == Some(1))
    }

    pub fn payloads_of_scope(conn: &Connection, scope: ScopeId) -> Result<Vec<PayloadId>> {
        let mut st = conn
            .prepare("SELECT id FROM payloads WHERE scope = ?1")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![scope.bytes().to_vec()], |r| r.get::<_, Vec<u8>>(0))
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|b| PayloadId::from_slice(&b))
            .collect();
        Ok(rows)
    }

    pub fn all_payloads(conn: &Connection) -> Result<Vec<(PayloadId, ScopeId, bool)>> {
        let mut st = conn
            .prepare("SELECT id, scope, present FROM payloads")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|(a, b, p)| {
                Some((PayloadId::from_slice(&a)?, ScopeId::from_slice(&b)?, p == 1))
            })
            .collect();
        Ok(rows)
    }

    /// Mark every payload of a scope absent (§10.5 commit step).
    pub fn set_scope_payloads_absent(conn: &Connection, scope: ScopeId) -> Result<()> {
        conn.execute(
            "UPDATE payloads SET present = 0 WHERE scope = ?1",
            params![scope.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    /// Delete a scope's staged rows and payload files (§10.5 Phase 2 step 1).
    pub fn shred_scope_payloads(&self, scope: ScopeId) -> Result<usize> {
        let ids = Self::payloads_of_scope(&self.conn, scope)?;
        let mut n = 0;
        for id in &ids {
            self.conn
                .execute(
                    "DELETE FROM staged_payloads WHERE id = ?1",
                    params![id.bytes().to_vec()],
                )
                .map_err(sql_err)?;
            let p = self.payloads_dir().join(Self::payload_rel_path(*id));
            match std::fs::remove_file(&p) {
                Ok(_) => n += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            let tmp = p.with_extension("tmp");
            let _ = std::fs::remove_file(tmp);
        }
        Ok(n)
    }

    // ---- scopes ----

    pub fn insert_scope(conn: &Connection, id: ScopeId, kind: &ScopeKind) -> Result<()> {
        let (concept, section, asset) = match kind {
            ScopeKind::Repo => (None, None, None),
            ScopeKind::Concept { concept } => (Some(concept.bytes().to_vec()), None, None),
            ScopeKind::Section { concept, section } => (
                Some(concept.bytes().to_vec()),
                Some(section.bytes().to_vec()),
                None,
            ),
            ScopeKind::Asset { asset } => (None, None, Some(asset.bytes().to_vec())),
            ScopeKind::Actor { .. } => (None, None, None),
        };
        conn.execute(
            "INSERT OR IGNORE INTO scopes (id, kind, concept, section, asset) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id.bytes().to_vec(), kind.as_str(), concept, section, asset],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    /// A moved section keeps its scope; update the owning concept for erase resolution (§10.4).
    pub fn set_scope_concept(conn: &Connection, id: ScopeId, concept: ConceptId) -> Result<()> {
        conn.execute(
            "UPDATE scopes SET concept = ?2 WHERE id = ?1",
            params![id.bytes().to_vec(), concept.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    pub fn scope(conn: &Connection, id: ScopeId) -> Result<Option<ScopeRow>> {
        conn.query_row(
            "SELECT id, kind, concept, section, asset, erased, shredded FROM scopes WHERE id = ?1",
            params![id.bytes().to_vec()],
            Self::row_to_scope,
        )
        .optional()
        .map_err(sql_err)
    }

    fn row_to_scope(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScopeRow> {
        let id = ScopeId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?));
        let kind: String = r.get(1)?;
        let concept: Option<Vec<u8>> = r.get(2)?;
        let section: Option<Vec<u8>> = r.get(3)?;
        let asset: Option<Vec<u8>> = r.get(4)?;
        let erased: i64 = r.get(5)?;
        let shredded: i64 = r.get(6)?;
        let kind = match kind.as_str() {
            "concept" => ScopeKind::Concept {
                concept: ConceptId::from_bytes(blob16(concept.unwrap_or_default())),
            },
            "section" => ScopeKind::Section {
                concept: ConceptId::from_bytes(blob16(concept.unwrap_or_default())),
                section: SectionId::from_bytes(blob16(section.unwrap_or_default())),
            },
            "asset" => ScopeKind::Asset {
                asset: AssetId::from_bytes(blob16(asset.unwrap_or_default())),
            },
            "actor" => ScopeKind::Actor {
                actor: ActorId::from_bytes([0u8; 16]),
            },
            _ => ScopeKind::Repo,
        };
        Ok(ScopeRow {
            id,
            kind,
            erased: erased == 1,
            shredded: shredded == 1,
        })
    }

    /// Scopes owned by a concept: the concept scope and every section scope it owns (§10.4).
    pub fn scopes_of_concept(conn: &Connection, concept: ConceptId) -> Result<Vec<ScopeRow>> {
        let mut st = conn
            .prepare("SELECT id, kind, concept, section, asset, erased, shredded FROM scopes WHERE concept = ?1")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![concept.bytes().to_vec()], Self::row_to_scope)
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn scope_of_section(conn: &Connection, section: SectionId) -> Result<Option<ScopeRow>> {
        conn.query_row(
            "SELECT id, kind, concept, section, asset, erased, shredded FROM scopes WHERE section = ?1",
            params![section.bytes().to_vec()],
            Self::row_to_scope,
        )
        .optional()
        .map_err(sql_err)
    }

    pub fn scope_of_asset(conn: &Connection, asset: AssetId) -> Result<Option<ScopeRow>> {
        conn.query_row(
            "SELECT id, kind, concept, section, asset, erased, shredded FROM scopes WHERE asset = ?1",
            params![asset.bytes().to_vec()],
            Self::row_to_scope,
        )
        .optional()
        .map_err(sql_err)
    }

    pub fn mark_scope_erased(conn: &Connection, id: ScopeId) -> Result<()> {
        conn.execute(
            "UPDATE scopes SET erased = 1 WHERE id = ?1",
            params![id.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn mark_scope_shredded(conn: &Connection, id: ScopeId) -> Result<()> {
        conn.execute(
            "UPDATE scopes SET shredded = 1 WHERE id = ?1",
            params![id.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn scopes_erased_not_shredded(conn: &Connection) -> Result<Vec<ScopeId>> {
        let mut st = conn
            .prepare("SELECT id FROM scopes WHERE erased = 1 AND shredded = 0")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| r.get::<_, Vec<u8>>(0))
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|b| ScopeId::from_slice(&b))
            .collect();
        Ok(rows)
    }
    pub fn all_scopes(conn: &Connection) -> Result<Vec<ScopeRow>> {
        let mut st = conn
            .prepare("SELECT id, kind, concept, section, asset, erased, shredded FROM scopes")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], Self::row_to_scope)
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    // ---- tombstones ----

    pub fn insert_tombstone(conn: &Connection, t: &Tombstone) -> Result<()> {
        conn.execute(
            "INSERT INTO tombstones (erase_change, target, scopes, reason, flagged) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                t.erase_change.bytes().to_vec(),
                serde_json::to_vec(&t.target)?,
                serde_json::to_vec(&t.scopes)?,
                t.reason.as_str(),
                serde_json::to_vec(&t.flagged)?,
            ],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    pub fn tombstones(conn: &Connection) -> Result<Vec<Tombstone>> {
        let mut st = conn
            .prepare("SELECT erase_change, target, scopes, reason, flagged FROM tombstones")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| {
                let ec: Vec<u8> = r.get(0)?;
                let target: Vec<u8> = r.get(1)?;
                let scopes: Vec<u8> = r.get(2)?;
                let reason: String = r.get(3)?;
                let flagged: Option<Vec<u8>> = r.get(4)?;
                Ok(Tombstone {
                    erase_change: ChangeId::from_bytes(blob16(ec)),
                    target: serde_json::from_slice(&target)
                        .unwrap_or(Target::Erase { scopes: vec![] }),
                    scopes: serde_json::from_slice(&scopes).unwrap_or_default(),
                    reason: Reason::parse(&reason).unwrap_or(Reason::Other),
                    flagged: flagged
                        .and_then(|b| serde_json::from_slice(&b).ok())
                        .unwrap_or_default(),
                })
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    // ---- actors ----

    pub fn insert_actor(conn: &Connection, a: &ActorRow) -> Result<()> {
        conn.execute(
            "INSERT INTO actors (id, kind, scope, name, public_key, name_index) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                a.id.bytes().to_vec(),
                a.kind.as_str(),
                a.scope.bytes().to_vec(),
                a.name.to_blob(),
                a.public_key.as_ref().map(|p| p.to_blob()),
                a.name_index.map(|n| n.to_vec()),
            ],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    fn row_to_actor(r: &rusqlite::Row<'_>) -> rusqlite::Result<ActorRow> {
        let id = ActorId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?));
        let kind: String = r.get(1)?;
        let scope = ScopeId::from_bytes(blob16(r.get::<_, Vec<u8>>(2)?));
        let name: Vec<u8> = r.get(3)?;
        let pk: Option<Vec<u8>> = r.get(4)?;
        let idx: Option<Vec<u8>> = r.get(5)?;
        Ok(ActorRow {
            id,
            kind: ActorKind::parse(&kind).unwrap_or(ActorKind::Process),
            scope,
            name: PayloadRef::from_blob(&name).unwrap_or(PayloadRef {
                id: PayloadId::from_bytes([0; 16]),
                commit: Commit([0; 32]),
            }),
            public_key: pk.and_then(|b| PayloadRef::from_blob(&b)),
            name_index: idx.and_then(|b| <[u8; 32]>::try_from(b).ok()),
        })
    }

    pub fn actor(conn: &Connection, id: ActorId) -> Result<Option<ActorRow>> {
        conn.query_row(
            "SELECT id, kind, scope, name, public_key, name_index FROM actors WHERE id = ?1",
            params![id.bytes().to_vec()],
            Self::row_to_actor,
        )
        .optional()
        .map_err(sql_err)
    }

    pub fn actor_by_index(conn: &Connection, idx: &[u8; 32]) -> Result<Option<ActorRow>> {
        conn.query_row(
            "SELECT id, kind, scope, name, public_key, name_index FROM actors WHERE name_index = ?1",
            params![idx.to_vec()],
            Self::row_to_actor,
        )
        .optional()
        .map_err(sql_err)
    }

    pub fn actors(conn: &Connection) -> Result<Vec<ActorRow>> {
        let mut st = conn
            .prepare("SELECT id, kind, scope, name, public_key, name_index FROM actors ORDER BY id")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], Self::row_to_actor)
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn set_actor_key(conn: &Connection, id: ActorId, key: &PayloadRef) -> Result<()> {
        conn.execute(
            "UPDATE actors SET public_key = ?2 WHERE id = ?1",
            params![id.bytes().to_vec(), key.to_blob()],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    /// Erasing an actor clears its name index (§10.4).
    pub fn clear_actor_index(conn: &Connection, id: ActorId) -> Result<()> {
        conn.execute(
            "UPDATE actors SET name_index = NULL WHERE id = ?1",
            params![id.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    pub fn actor_scope(conn: &Connection, id: ActorId) -> Result<Option<ScopeId>> {
        Ok(Self::actor(conn, id)?.map(|a| a.scope))
    }

    // ---- groups ----

    pub fn insert_group(conn: &Connection, id: GroupId, label: &PayloadRef) -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO groups (id, label) VALUES (?1, ?2)",
            params![id.bytes().to_vec(), label.to_blob()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn groups(conn: &Connection) -> Result<Vec<(GroupId, PayloadRef)>> {
        let mut st = conn
            .prepare("SELECT id, label FROM groups")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|(a, b)| Some((GroupId::from_slice(&a)?, PayloadRef::from_blob(&b)?)))
            .collect();
        Ok(rows)
    }
    pub fn group_add_member(
        conn: &Connection,
        group: GroupId,
        member: &[u8],
        kind: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO group_members (group_id, member, kind) VALUES (?1, ?2, ?3)",
            params![group.bytes().to_vec(), member.to_vec(), kind],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn group_remove_member(conn: &Connection, group: GroupId, member: &[u8]) -> Result<()> {
        conn.execute(
            "DELETE FROM group_members WHERE group_id = ?1 AND member = ?2",
            params![group.bytes().to_vec(), member.to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn group_members(conn: &Connection, group: GroupId) -> Result<Vec<(Vec<u8>, String)>> {
        let mut st = conn
            .prepare("SELECT member, kind FROM group_members WHERE group_id = ?1")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![group.bytes().to_vec()], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    // ---- erasure markers (bookkeeping) ----

    pub fn insert_erasure_marker(
        conn: &Connection,
        section: SectionId,
        concept: ConceptId,
        after: Option<SectionId>,
        reason: Reason,
    ) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO erasure_markers (section, concept, after_section, reason) VALUES (?1, ?2, ?3, ?4)",
            params![section.bytes().to_vec(), concept.bytes().to_vec(), after.map(|a| a.bytes().to_vec()), reason.as_str()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn erasure_markers(conn: &Connection) -> Result<Vec<ErasureMarkerRow>> {
        let mut st = conn
            .prepare("SELECT section, concept, after_section, reason FROM erasure_markers")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Option<Vec<u8>>>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|(s, c, a, reason)| {
                Some((
                    SectionId::from_slice(&s)?,
                    ConceptId::from_slice(&c)?,
                    a.and_then(|b| SectionId::from_slice(&b)),
                    Reason::parse(&reason).unwrap_or(Reason::Other),
                ))
            })
            .collect();
        Ok(rows)
    }

    /// Every change in ledger order (used for asset history and fsck).
    pub fn changes_all(conn: &Connection) -> Result<Vec<(i64, Change)>> {
        Self::changes_since(conn, 0)
    }

    /// Erase changes with no tombstone row yet (recovery bookkeeping).
    pub fn erase_changes_without_tombstones(conn: &Connection) -> Result<Vec<Change>> {
        let sql = format!(
            "SELECT {} FROM changes c
             JOIN changesets s ON s.id = c.changeset
             WHERE c.op = 'erase'
               AND NOT EXISTS (SELECT 1 FROM tombstones t WHERE t.erase_change = c.id)
             ORDER BY s.seq, c.ordinal",
            Self::CHANGE_COLS
        );
        let mut st = conn.prepare(&sql).map_err(sql_err)?;
        let rows = st
            .query_map([], Self::row_to_change)
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    const ERASE_REWRITE_PENDING: &'static str = "erase_rewrite_pending";

    pub fn erase_rewrite_pending(conn: &Connection) -> Result<bool> {
        Ok(conn
            .query_row(
                "SELECT value FROM ledger_flags WHERE name = ?1",
                params![Self::ERASE_REWRITE_PENDING],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(sql_err)?
            .unwrap_or(0)
            != 0)
    }

    pub fn set_erase_rewrite_pending(conn: &Connection, pending: bool) -> Result<()> {
        conn.execute(
            "INSERT INTO ledger_flags (name, value) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            params![Self::ERASE_REWRITE_PENDING, if pending { 1 } else { 0 }],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    // ---- merge replay skips (§11.5) ----

    pub fn set_merge_replay_skips(
        conn: &Connection,
        ref_name: &str,
        merge_changeset: ChangesetId,
        skip: &[ChangeId],
    ) -> Result<()> {
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(sql_err)?;
        let outcome = (|| {
            conn.execute(
                "DELETE FROM merge_replay_skips WHERE ref_name = ?1 AND merge_changeset = ?2",
                params![ref_name, merge_changeset.bytes().to_vec()],
            )
            .map_err(sql_err)?;
            for c in skip {
                conn.execute(
                    "INSERT INTO merge_replay_skips (ref_name, merge_changeset, change_id) VALUES (?1, ?2, ?3)",
                    params![
                        ref_name,
                        merge_changeset.bytes().to_vec(),
                        c.bytes().to_vec()
                    ],
                )
                .map_err(sql_err)?;
            }
            Ok(())
        })();
        if outcome.is_err() {
            let _ = conn.execute_batch("ROLLBACK");
            return outcome;
        }
        conn.execute_batch("COMMIT").map_err(sql_err)?;
        Ok(())
    }

    pub fn merge_replay_skips(
        conn: &Connection,
        ref_name: &str,
        merge_changeset: ChangesetId,
    ) -> Result<Vec<ChangeId>> {
        let mut st = conn
            .prepare(
                "SELECT change_id FROM merge_replay_skips WHERE ref_name = ?1 AND merge_changeset = ?2",
            )
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![ref_name, merge_changeset.bytes().to_vec()], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|b| ChangeId::from_slice(&b))
            .collect();
        Ok(rows)
    }

    pub fn clear_merge_replay_skips(conn: &Connection, ref_name: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM merge_replay_skips WHERE ref_name = ?1",
            params![ref_name],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    // ---- cursors and pending erasures ----

    pub fn cursor(conn: &Connection, subscriber: &str) -> Result<i64> {
        Ok(conn
            .query_row(
                "SELECT seq FROM cursors WHERE subscriber = ?1",
                params![subscriber],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)?
            .unwrap_or(0))
    }
    pub fn set_cursor(conn: &Connection, subscriber: &str, seq: i64) -> Result<()> {
        conn.execute(
            "INSERT INTO cursors (subscriber, seq) VALUES (?1, ?2) ON CONFLICT(subscriber) DO UPDATE SET seq = excluded.seq",
            params![subscriber, seq],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn add_pending_erasure(
        conn: &Connection,
        change: ChangeId,
        subscriber: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO pending_erasures (erase_change, subscriber) VALUES (?1, ?2)",
            params![change.bytes().to_vec(), subscriber],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn clear_pending_erasure(
        conn: &Connection,
        change: ChangeId,
        subscriber: &str,
    ) -> Result<()> {
        conn.execute(
            "DELETE FROM pending_erasures WHERE erase_change = ?1 AND subscriber = ?2",
            params![change.bytes().to_vec(), subscriber],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn pending_erasures(conn: &Connection) -> Result<Vec<(ChangeId, String)>> {
        let mut st = conn
            .prepare("SELECT erase_change, subscriber FROM pending_erasures")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|(a, s)| Some((ChangeId::from_slice(&a)?, s)))
            .collect();
        Ok(rows)
    }

    pub fn checkpoint(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA incremental_vacuum; PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(sql_err)?;
        Ok(())
    }
}

fn payload_kind_from_str(s: &str) -> PayloadKind {
    match s {
        "section" => PayloadKind::Section,
        "note" => PayloadKind::Note,
        "path" => PayloadKind::Path,
        "frontmatter_entry" => PayloadKind::FrontmatterEntry,
        "source_record" => PayloadKind::SourceRecord,
        "asset_bytes" => PayloadKind::AssetBytes,
        "message" => PayloadKind::Message,
        "group_label" => PayloadKind::GroupLabel,
        "actor_name" => PayloadKind::ActorName,
        "actor_key" => PayloadKind::ActorKey,
        "section_order" => PayloadKind::SectionOrder,
        "status" => PayloadKind::Status,
        "group_member" => PayloadKind::GroupMember,
        _ => PayloadKind::Message,
    }
}

fn ulid_suffix() -> String {
    PayloadId::new().to_string()
}

#[allow(dead_code)]
type Result<T> = SqlResult<T>;
