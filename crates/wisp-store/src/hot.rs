//! The hot tier (§6.2): a plaintext, indexed copy of the checked-out, non-retracted state.
//! A cache: always rebuildable from the cold tier and the keystore.

use crate::{blob16, sql_err, SqlResult as Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use wisp_model::*;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS hot_meta (
  ref        TEXT NOT NULL,
  head       BLOB NOT NULL,
  ledger_seq INTEGER NOT NULL,
  profile    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS state_concepts (
  concept BLOB PRIMARY KEY,
  path    TEXT NOT NULL UNIQUE,
  type    TEXT,
  status  TEXT NOT NULL,
  scope   BLOB NOT NULL,
  version BLOB NOT NULL,
  gen_actor BLOB,
  gen_at    TEXT,
  content_version BLOB
);
CREATE INDEX IF NOT EXISTS state_concepts_scope ON state_concepts(scope);
CREATE TABLE IF NOT EXISTS state_aliases (
  concept BLOB NOT NULL, path TEXT NOT NULL, until_changeset BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS state_aliases_path ON state_aliases(path);
CREATE TABLE IF NOT EXISTS state_sections (
  rowid   INTEGER PRIMARY KEY,
  concept BLOB NOT NULL REFERENCES state_concepts(concept),
  section BLOB NOT NULL,
  idx     INTEGER NOT NULL,
  level   INTEGER NOT NULL,
  heading TEXT NOT NULL,
  heading_line TEXT NOT NULL,
  body    TEXT NOT NULL,
  scope   BLOB NOT NULL,
  version BLOB NOT NULL,
  UNIQUE (concept, section)
);
CREATE INDEX IF NOT EXISTS state_sections_scope ON state_sections(scope);
CREATE TABLE IF NOT EXISTS state_frontmatter (
  concept BLOB NOT NULL, idx INTEGER NOT NULL,
  key TEXT NOT NULL, raw TEXT NOT NULL, managed INTEGER NOT NULL,
  version BLOB NOT NULL,
  PRIMARY KEY (concept, idx)
);
CREATE TABLE IF NOT EXISTS state_sources (
  concept BLOB NOT NULL, source_id TEXT NOT NULL, record TEXT NOT NULL,
  version BLOB NOT NULL,
  idx INTEGER NOT NULL, PRIMARY KEY (concept, source_id)
);
CREATE TABLE IF NOT EXISTS state_assets (
  asset BLOB PRIMARY KEY, path TEXT NOT NULL UNIQUE, bytes BLOB NOT NULL,
  scope BLOB NOT NULL, version BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS state_links (
  concept BLOB NOT NULL, section BLOB NOT NULL,
  target_concept BLOB,
  target_path TEXT,
  form TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS state_links_target ON state_links(target_concept);
CREATE TABLE IF NOT EXISTS state_flags (
  concept BLOB NOT NULL, cause_change BLOB NOT NULL, kind TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS state_retracted (
  concept BLOB PRIMARY KEY, path TEXT NOT NULL, at TEXT NOT NULL, actor BLOB NOT NULL, reason TEXT NOT NULL, kind TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS state_verified (
  concept BLOB NOT NULL, actor BLOB NOT NULL, at TEXT NOT NULL, content_version BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS state_erased_markers (
  concept BLOB NOT NULL, section BLOB NOT NULL, after_section BLOB, reason TEXT NOT NULL,
  PRIMARY KEY (concept, section)
);

CREATE VIRTUAL TABLE IF NOT EXISTS state_search_fts USING fts5(
  heading, body,
  content = 'state_sections', content_rowid = 'rowid',
  tokenize = 'porter unicode61'
);
CREATE TRIGGER IF NOT EXISTS fts_ai AFTER INSERT ON state_sections BEGIN
  INSERT INTO state_search_fts(rowid, heading, body) VALUES (new.rowid, new.heading, new.body);
END;
CREATE TRIGGER IF NOT EXISTS fts_ad AFTER DELETE ON state_sections BEGIN
  INSERT INTO state_search_fts(state_search_fts, rowid, heading, body) VALUES ('delete', old.rowid, old.heading, old.body);
END;
CREATE TRIGGER IF NOT EXISTS fts_au AFTER UPDATE ON state_sections BEGIN
  INSERT INTO state_search_fts(state_search_fts, rowid, heading, body) VALUES ('delete', old.rowid, old.heading, old.body);
  INSERT INTO state_search_fts(rowid, heading, body) VALUES (new.rowid, new.heading, new.body);
END;

CREATE VIRTUAL TABLE IF NOT EXISTS state_search_trigram USING fts5(
  heading, body,
  content = 'state_sections', content_rowid = 'rowid',
  tokenize = 'trigram'
);
CREATE TRIGGER IF NOT EXISTS fts_tri_ai AFTER INSERT ON state_sections BEGIN
  INSERT INTO state_search_trigram(rowid, heading, body) VALUES (new.rowid, new.heading, new.body);
END;
CREATE TRIGGER IF NOT EXISTS fts_tri_ad AFTER DELETE ON state_sections BEGIN
  INSERT INTO state_search_trigram(state_search_trigram, rowid, heading, body) VALUES ('delete', old.rowid, old.heading, old.body);
END;
CREATE TRIGGER IF NOT EXISTS fts_tri_au AFTER UPDATE ON state_sections BEGIN
  INSERT INTO state_search_trigram(state_search_trigram, rowid, heading, body) VALUES ('delete', old.rowid, old.heading, old.body);
  INSERT INTO state_search_trigram(rowid, heading, body) VALUES (new.rowid, new.heading, new.body);
END;

CREATE TABLE IF NOT EXISTS section_dense_embeddings (
  section BLOB PRIMARY KEY,
  model_tag TEXT NOT NULL,
  dimensions INTEGER NOT NULL,
  vector BLOB NOT NULL
);
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConceptState {
    pub concept: ConceptId,
    pub path: String,
    pub type_: Option<String>,
    /// None when `set_status` was never applied (OKF reads that as `stable`).
    pub status: Option<Status>,
    pub scope: ScopeId,
    pub version: ChangeId,
    pub gen_actor: Option<ActorId>,
    pub gen_at: Option<String>,
    pub content_version: Option<ChangeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionState {
    pub concept: ConceptId,
    pub section: SectionId,
    pub idx: i64,
    pub level: u8,
    pub heading: String,
    pub heading_line: String,
    pub body: String,
    pub scope: ScopeId,
    pub version: ChangeId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontmatterState {
    pub idx: i64,
    pub key: String,
    pub raw: String,
    pub managed: bool,
    pub version: ChangeId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceState {
    pub source_id: String,
    pub record: serde_json::Value,
    pub version: ChangeId,
    pub idx: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetState {
    pub asset: AssetId,
    pub path: String,
    pub bytes: Vec<u8>,
    pub scope: ScopeId,
    pub version: ChangeId,
}

#[derive(Clone, Debug)]
pub struct HotMeta {
    pub ref_name: String,
    pub head: ChangesetId,
    pub ledger_seq: i64,
    pub profile: String,
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub concept: ConceptId,
    pub section: SectionId,
    pub path: String,
    pub heading: String,
    pub score: f64,
    pub body: String,
}

pub struct Hot {
    conn: Connection,
    path: PathBuf,
}

/// `ref-id` is the hex SHA-256 of the ref name (§6.3).
pub fn ref_id(ref_name: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(ref_name.as_bytes()))
}

impl Hot {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path).map_err(sql_err)?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .map_err(sql_err)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA secure_delete = ON; PRAGMA auto_vacuum = INCREMENTAL; PRAGMA synchronous = NORMAL;")
            .map_err(sql_err)?;
        conn.execute_batch(SCHEMA).map_err(sql_err)?;
        conn.execute_batch(crate::ri::schema_sql())
            .map_err(sql_err)?;
        // FTS5 secure-delete mode removes deleted tokens immediately (§6.2).
        let _ = conn.execute_batch(
            "INSERT INTO state_search_fts(state_search_fts, rank) VALUES ('secure-delete', 1);
             INSERT INTO state_search_trigram(state_search_trigram, rank) VALUES ('secure-delete', 1);",
        );
        Ok(Hot {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn begin(&mut self) -> Result<Transaction<'_>> {
        self.conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(sql_err)
    }

    // ---- meta ----

    pub fn meta(conn: &Connection) -> Result<Option<HotMeta>> {
        conn.query_row(
            "SELECT ref, head, ledger_seq, profile FROM hot_meta LIMIT 1",
            [],
            |r| {
                Ok(HotMeta {
                    ref_name: r.get(0)?,
                    head: ChangesetId::from_bytes(blob16(r.get::<_, Vec<u8>>(1)?)),
                    ledger_seq: r.get(2)?,
                    profile: r.get(3)?,
                })
            },
        )
        .optional()
        .map_err(sql_err)
    }
    pub fn set_meta(conn: &Connection, m: &HotMeta) -> Result<()> {
        conn.execute("DELETE FROM hot_meta", []).map_err(sql_err)?;
        conn.execute(
            "INSERT INTO hot_meta (ref, head, ledger_seq, profile) VALUES (?1, ?2, ?3, ?4)",
            params![m.ref_name, m.head.bytes().to_vec(), m.ledger_seq, m.profile],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn advance(conn: &Connection, head: ChangesetId, seq: i64) -> Result<()> {
        conn.execute(
            "UPDATE hot_meta SET head = ?1, ledger_seq = ?2",
            params![head.bytes().to_vec(), seq],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    /// Wipe all state tables (for rebuild).
    pub fn clear(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "DELETE FROM state_links; DELETE FROM state_flags; DELETE FROM state_sections; DELETE FROM state_frontmatter;
             DELETE FROM state_sources; DELETE FROM state_assets; DELETE FROM state_aliases; DELETE FROM state_retracted;
             DELETE FROM state_verified; DELETE FROM state_erased_markers; DELETE FROM state_concepts; DELETE FROM hot_meta;",
        )
        .map_err(sql_err)?;
        conn.execute("DELETE FROM section_dense_embeddings", [])
            .map_err(sql_err)?;
        crate::ri::clear(conn)?;
        Ok(())
    }

    // ---- concepts ----

    fn row_to_concept(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConceptState> {
        Ok(ConceptState {
            concept: ConceptId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?)),
            path: r.get(1)?,
            type_: r.get(2)?,
            status: Status::parse(&r.get::<_, String>(3)?),
            scope: ScopeId::from_bytes(blob16(r.get::<_, Vec<u8>>(4)?)),
            version: ChangeId::from_bytes(blob16(r.get::<_, Vec<u8>>(5)?)),
            gen_actor: r
                .get::<_, Option<Vec<u8>>>(6)?
                .and_then(|b| ActorId::from_slice(&b)),
            gen_at: r.get(7)?,
            content_version: r
                .get::<_, Option<Vec<u8>>>(8)?
                .and_then(|b| ChangeId::from_slice(&b)),
        })
    }
    const CONCEPT_COLS: &'static str =
        "concept, path, type, status, scope, version, gen_actor, gen_at, content_version";

    pub fn concept(conn: &Connection, id: ConceptId) -> Result<Option<ConceptState>> {
        let sql = format!(
            "SELECT {} FROM state_concepts WHERE concept = ?1",
            Self::CONCEPT_COLS
        );
        conn.query_row(&sql, params![id.bytes().to_vec()], Self::row_to_concept)
            .optional()
            .map_err(sql_err)
    }
    pub fn concept_by_path(conn: &Connection, path: &str) -> Result<Option<ConceptState>> {
        let sql = format!(
            "SELECT {} FROM state_concepts WHERE path = ?1",
            Self::CONCEPT_COLS
        );
        conn.query_row(&sql, params![path], Self::row_to_concept)
            .optional()
            .map_err(sql_err)
    }
    pub fn concept_by_alias(conn: &Connection, path: &str) -> Result<Option<ConceptState>> {
        let id: Option<Vec<u8>> = conn
            .query_row("SELECT concept FROM state_aliases WHERE path = ?1 ORDER BY until_changeset DESC LIMIT 1", params![path], |r| r.get(0))
            .optional()
            .map_err(sql_err)?;
        match id.and_then(|b| ConceptId::from_slice(&b)) {
            Some(c) => Self::concept(conn, c),
            None => Ok(None),
        }
    }
    pub fn concepts(conn: &Connection) -> Result<Vec<ConceptState>> {
        let sql = format!(
            "SELECT {} FROM state_concepts ORDER BY path",
            Self::CONCEPT_COLS
        );
        let mut st = conn.prepare(&sql).map_err(sql_err)?;
        let rows = st
            .query_map([], Self::row_to_concept)
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
    pub fn upsert_concept(conn: &Connection, c: &ConceptState) -> Result<()> {
        conn.execute(
            "INSERT INTO state_concepts (concept, path, type, status, scope, version, gen_actor, gen_at, content_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(concept) DO UPDATE SET path = excluded.path, type = excluded.type, status = excluded.status,
               scope = excluded.scope, version = excluded.version, gen_actor = excluded.gen_actor, gen_at = excluded.gen_at,
               content_version = excluded.content_version",
            params![
                c.concept.bytes().to_vec(),
                c.path,
                c.type_,
                c.status.map(|s| s.as_str()).unwrap_or("unset"),
                c.scope.bytes().to_vec(),
                c.version.bytes().to_vec(),
                c.gen_actor.map(|a| a.bytes().to_vec()),
                c.gen_at,
                c.content_version.map(|v| v.bytes().to_vec()),
            ],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn delete_concept(conn: &Connection, id: ConceptId) -> Result<()> {
        let b = id.bytes().to_vec();
        conn.execute("DELETE FROM state_links WHERE concept = ?1", params![b])
            .map_err(sql_err)?;
        conn.execute("DELETE FROM state_sections WHERE concept = ?1", params![b])
            .map_err(sql_err)?;
        conn.execute(
            "DELETE FROM state_frontmatter WHERE concept = ?1",
            params![b],
        )
        .map_err(sql_err)?;
        conn.execute("DELETE FROM state_sources WHERE concept = ?1", params![b])
            .map_err(sql_err)?;
        conn.execute("DELETE FROM state_verified WHERE concept = ?1", params![b])
            .map_err(sql_err)?;
        conn.execute(
            "DELETE FROM state_erased_markers WHERE concept = ?1",
            params![b],
        )
        .map_err(sql_err)?;
        conn.execute("DELETE FROM state_flags WHERE concept = ?1", params![b])
            .map_err(sql_err)?;
        conn.execute("DELETE FROM state_concepts WHERE concept = ?1", params![b])
            .map_err(sql_err)?;
        Ok(())
    }
    pub fn add_alias(
        conn: &Connection,
        concept: ConceptId,
        path: &str,
        until: ChangesetId,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO state_aliases (concept, path, until_changeset) VALUES (?1, ?2, ?3)",
            params![concept.bytes().to_vec(), path, until.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn aliases(conn: &Connection, concept: ConceptId) -> Result<Vec<String>> {
        let mut st = conn
            .prepare("SELECT path FROM state_aliases WHERE concept = ?1")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![concept.bytes().to_vec()], |r| r.get::<_, String>(0))
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    // ---- sections ----

    fn row_to_section(r: &rusqlite::Row<'_>) -> rusqlite::Result<SectionState> {
        Ok(SectionState {
            concept: ConceptId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?)),
            section: SectionId::from_bytes(blob16(r.get::<_, Vec<u8>>(1)?)),
            idx: r.get(2)?,
            level: r.get::<_, i64>(3)? as u8,
            heading: r.get(4)?,
            heading_line: r.get(5)?,
            body: r.get(6)?,
            scope: ScopeId::from_bytes(blob16(r.get::<_, Vec<u8>>(7)?)),
            version: ChangeId::from_bytes(blob16(r.get::<_, Vec<u8>>(8)?)),
        })
    }
    const SECTION_COLS: &'static str =
        "concept, section, idx, level, heading, heading_line, body, scope, version";

    pub fn sections_of(conn: &Connection, concept: ConceptId) -> Result<Vec<SectionState>> {
        let sql = format!(
            "SELECT {} FROM state_sections WHERE concept = ?1 ORDER BY idx",
            Self::SECTION_COLS
        );
        let mut st = conn.prepare(&sql).map_err(sql_err)?;
        let rows = st
            .query_map(params![concept.bytes().to_vec()], Self::row_to_section)
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
    pub fn section(conn: &Connection, section: SectionId) -> Result<Option<SectionState>> {
        let sql = format!(
            "SELECT {} FROM state_sections WHERE section = ?1",
            Self::SECTION_COLS
        );
        conn.query_row(
            &sql,
            params![section.bytes().to_vec()],
            Self::row_to_section,
        )
        .optional()
        .map_err(sql_err)
    }
    pub fn upsert_section(conn: &Connection, s: &SectionState) -> Result<()> {
        conn.execute(
            "INSERT INTO state_sections (concept, section, idx, level, heading, heading_line, body, scope, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(concept, section) DO UPDATE SET idx = excluded.idx, level = excluded.level, heading = excluded.heading,
               heading_line = excluded.heading_line, body = excluded.body, scope = excluded.scope, version = excluded.version",
            params![
                s.concept.bytes().to_vec(),
                s.section.bytes().to_vec(),
                s.idx,
                s.level as i64,
                s.heading,
                s.heading_line,
                s.body,
                s.scope.bytes().to_vec(),
                s.version.bytes().to_vec(),
            ],
        )
        .map_err(sql_err)?;
        crate::ri::reindex_section(conn, s)?;
        conn.execute(
            "DELETE FROM section_dense_embeddings WHERE section = ?1",
            params![s.section.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn delete_section(conn: &Connection, section: SectionId) -> Result<()> {
        crate::ri::unindex_section(conn, section)?;
        conn.execute(
            "DELETE FROM state_links WHERE section = ?1",
            params![section.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        conn.execute(
            "DELETE FROM section_dense_embeddings WHERE section = ?1",
            params![section.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        conn.execute(
            "DELETE FROM state_sections WHERE section = ?1",
            params![section.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    /// Renumber `idx` to match `order`; sections not in `order` are appended in their current order.
    pub fn set_section_order(
        conn: &Connection,
        concept: ConceptId,
        order: &[SectionId],
    ) -> Result<()> {
        let current = Self::sections_of(conn, concept)?;
        let mut idx = 0i64;
        for s in order {
            conn.execute(
                "UPDATE state_sections SET idx = ?3 WHERE concept = ?1 AND section = ?2",
                params![concept.bytes().to_vec(), s.bytes().to_vec(), idx],
            )
            .map_err(sql_err)?;
            idx += 1;
        }
        for s in current.iter().filter(|s| !order.contains(&s.section)) {
            conn.execute(
                "UPDATE state_sections SET idx = ?3 WHERE concept = ?1 AND section = ?2",
                params![concept.bytes().to_vec(), s.section.bytes().to_vec(), idx],
            )
            .map_err(sql_err)?;
            idx += 1;
        }
        Ok(())
    }
    /// Insert a section after `after` (None = at the start, after the preamble if any).
    /// Returns false when `after` was not found (the section is appended).
    pub fn insert_section_after(
        conn: &Connection,
        s: &SectionState,
        after: Option<SectionId>,
    ) -> Result<bool> {
        let current = Self::sections_of(conn, s.concept)?;
        let mut order: Vec<SectionId> = current.iter().map(|x| x.section).collect();
        let (pos, found) = match after {
            None => (
                if current.first().map(|x| x.level == 0).unwrap_or(false) {
                    1
                } else {
                    0
                },
                true,
            ),
            Some(a) => match order.iter().position(|x| *x == a) {
                Some(p) => (p + 1, true),
                None => (order.len(), false),
            },
        };
        order.insert(pos.min(order.len()), s.section);
        let mut s2 = s.clone();
        s2.idx = order.len() as i64; // temporary; renumbered below
        Self::upsert_section(conn, &s2)?;
        Self::set_section_order(conn, s.concept, &order)?;
        Ok(found)
    }

    // ---- frontmatter ----

    pub fn frontmatter_of(conn: &Connection, concept: ConceptId) -> Result<Vec<FrontmatterState>> {
        let mut st = conn
            .prepare("SELECT idx, key, raw, managed, version FROM state_frontmatter WHERE concept = ?1 ORDER BY idx")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![concept.bytes().to_vec()], |r| {
                Ok(FrontmatterState {
                    idx: r.get(0)?,
                    key: r.get(1)?,
                    raw: r.get(2)?,
                    managed: r.get::<_, i64>(3)? == 1,
                    version: ChangeId::from_bytes(blob16(r.get::<_, Vec<u8>>(4)?)),
                })
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
    pub fn frontmatter_entry(
        conn: &Connection,
        concept: ConceptId,
        key: &str,
    ) -> Result<Option<FrontmatterState>> {
        Ok(Self::frontmatter_of(conn, concept)?
            .into_iter()
            .find(|e| e.key == key))
    }
    /// Set (replace in place or append) one entry.
    pub fn set_frontmatter(
        conn: &Connection,
        concept: ConceptId,
        key: &str,
        raw: &str,
        version: ChangeId,
    ) -> Result<()> {
        let existing = Self::frontmatter_of(conn, concept)?;
        if let Some(e) = existing.iter().find(|e| e.key == key) {
            conn.execute(
                "UPDATE state_frontmatter SET raw = ?3, version = ?4 WHERE concept = ?1 AND idx = ?2",
                params![concept.bytes().to_vec(), e.idx, raw, version.bytes().to_vec()],
            )
            .map_err(sql_err)?;
        } else {
            let idx = existing.iter().map(|e| e.idx).max().unwrap_or(-1) + 1;
            conn.execute(
                "INSERT INTO state_frontmatter (concept, idx, key, raw, managed, version) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![concept.bytes().to_vec(), idx, key, raw, version.bytes().to_vec()],
            )
            .map_err(sql_err)?;
        }
        Ok(())
    }
    pub fn unset_frontmatter(conn: &Connection, concept: ConceptId, key: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM state_frontmatter WHERE concept = ?1 AND key = ?2",
            params![concept.bytes().to_vec(), key],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    // ---- sources ----

    pub fn sources_of(conn: &Connection, concept: ConceptId) -> Result<Vec<SourceState>> {
        let mut st = conn
            .prepare("SELECT source_id, record, version, idx FROM state_sources WHERE concept = ?1 ORDER BY idx")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![concept.bytes().to_vec()], |r| {
                Ok(SourceState {
                    source_id: r.get(0)?,
                    record: serde_json::from_str(&r.get::<_, String>(1)?)
                        .unwrap_or(serde_json::Value::Null),
                    version: ChangeId::from_bytes(blob16(r.get::<_, Vec<u8>>(2)?)),
                    idx: r.get(3)?,
                })
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
    pub fn put_source(
        conn: &Connection,
        concept: ConceptId,
        id: &str,
        record: &serde_json::Value,
        version: ChangeId,
    ) -> Result<()> {
        let existing = Self::sources_of(conn, concept)?;
        let idx = existing
            .iter()
            .find(|s| s.source_id == id)
            .map(|s| s.idx)
            .unwrap_or_else(|| existing.iter().map(|s| s.idx).max().unwrap_or(-1) + 1);
        conn.execute(
            "INSERT INTO state_sources (concept, source_id, record, version, idx) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(concept, source_id) DO UPDATE SET record = excluded.record, version = excluded.version",
            params![concept.bytes().to_vec(), id, serde_json::to_string(record)?, version.bytes().to_vec(), idx],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn remove_source(conn: &Connection, concept: ConceptId, id: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM state_sources WHERE concept = ?1 AND source_id = ?2",
            params![concept.bytes().to_vec(), id],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    // ---- assets ----

    pub fn asset_by_path(conn: &Connection, path: &str) -> Result<Option<AssetState>> {
        conn.query_row(
            "SELECT asset, path, bytes, scope, version FROM state_assets WHERE path = ?1",
            params![path],
            Self::row_to_asset,
        )
        .optional()
        .map_err(sql_err)
    }
    pub fn asset(conn: &Connection, id: AssetId) -> Result<Option<AssetState>> {
        conn.query_row(
            "SELECT asset, path, bytes, scope, version FROM state_assets WHERE asset = ?1",
            params![id.bytes().to_vec()],
            Self::row_to_asset,
        )
        .optional()
        .map_err(sql_err)
    }
    pub fn assets(conn: &Connection) -> Result<Vec<AssetState>> {
        let mut st = conn
            .prepare("SELECT asset, path, bytes, scope, version FROM state_assets ORDER BY path")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], Self::row_to_asset)
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
    fn row_to_asset(r: &rusqlite::Row<'_>) -> rusqlite::Result<AssetState> {
        Ok(AssetState {
            asset: AssetId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?)),
            path: r.get(1)?,
            bytes: r.get(2)?,
            scope: ScopeId::from_bytes(blob16(r.get::<_, Vec<u8>>(3)?)),
            version: ChangeId::from_bytes(blob16(r.get::<_, Vec<u8>>(4)?)),
        })
    }
    pub fn upsert_asset(conn: &Connection, a: &AssetState) -> Result<()> {
        conn.execute(
            "INSERT INTO state_assets (asset, path, bytes, scope, version) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(asset) DO UPDATE SET path = excluded.path, bytes = excluded.bytes, scope = excluded.scope, version = excluded.version",
            params![a.asset.bytes().to_vec(), a.path, a.bytes, a.scope.bytes().to_vec(), a.version.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn delete_asset(conn: &Connection, id: AssetId) -> Result<()> {
        conn.execute(
            "DELETE FROM state_assets WHERE asset = ?1",
            params![id.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    // ---- links, flags ----

    pub fn replace_links(
        conn: &Connection,
        concept: ConceptId,
        section: SectionId,
        links: &[(Option<ConceptId>, Option<String>, String)],
    ) -> Result<()> {
        conn.execute(
            "DELETE FROM state_links WHERE concept = ?1 AND section = ?2",
            params![concept.bytes().to_vec(), section.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        for (target, path, form) in links {
            conn.execute(
                "INSERT INTO state_links (concept, section, target_concept, target_path, form) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![concept.bytes().to_vec(), section.bytes().to_vec(), target.map(|t| t.bytes().to_vec()), path, form],
            )
            .map_err(sql_err)?;
        }
        Ok(())
    }
    pub fn backlinks(conn: &Connection, target: ConceptId) -> Result<Vec<(ConceptId, SectionId)>> {
        let mut st = conn
            .prepare("SELECT concept, section FROM state_links WHERE target_concept = ?1")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![target.bytes().to_vec()], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|(a, b)| Some((ConceptId::from_slice(&a)?, SectionId::from_slice(&b)?)))
            .collect();
        Ok(rows)
    }
    /// Links to an erased concept become unresolved with no target path (§10.5 Phase 1).
    pub fn unresolve_links_to(conn: &Connection, target: ConceptId) -> Result<()> {
        conn.execute("UPDATE state_links SET target_concept = NULL, target_path = NULL WHERE target_concept = ?1", params![target.bytes().to_vec()])
            .map_err(sql_err)?;
        Ok(())
    }
    /// Links to a retracted concept become wanted again, keeping the path so restore can re-resolve them.
    pub fn unresolve_links_to_path(conn: &Connection, target: ConceptId, path: &str) -> Result<()> {
        conn.execute(
            "UPDATE state_links SET target_concept = NULL, target_path = ?2 WHERE target_concept = ?1",
            params![target.bytes().to_vec(), path],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    /// Wanted targets: unresolved link paths.
    pub fn wanted(conn: &Connection) -> Result<Vec<String>> {
        let mut st = conn.prepare("SELECT DISTINCT target_path FROM state_links WHERE target_concept IS NULL AND target_path IS NOT NULL").map_err(sql_err)?;
        let rows = st
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
    pub fn add_flag(
        conn: &Connection,
        concept: ConceptId,
        cause: ChangeId,
        kind: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO state_flags (concept, cause_change, kind) VALUES (?1, ?2, ?3)",
            params![concept.bytes().to_vec(), cause.bytes().to_vec(), kind],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn flags(conn: &Connection, concept: ConceptId) -> Result<Vec<(ChangeId, String)>> {
        let mut st = conn
            .prepare("SELECT cause_change, kind FROM state_flags WHERE concept = ?1")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![concept.bytes().to_vec()], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|(a, k)| Some((ChangeId::from_slice(&a)?, k)))
            .collect();
        Ok(rows)
    }

    // ---- retracted, verified, erased markers ----

    pub fn set_retracted(
        conn: &Connection,
        concept: ConceptId,
        path: &str,
        at: &str,
        actor: ActorId,
        reason: &str,
        kind: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO state_retracted (concept, path, at, actor, reason, kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![concept.bytes().to_vec(), path, at, actor.bytes().to_vec(), reason, kind],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn clear_retracted(conn: &Connection, concept: ConceptId) -> Result<()> {
        conn.execute(
            "DELETE FROM state_retracted WHERE concept = ?1",
            params![concept.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn retracted(
        conn: &Connection,
        concept: ConceptId,
    ) -> Result<Option<(String, String, ActorId, String)>> {
        conn.query_row(
            "SELECT path, at, actor, reason FROM state_retracted WHERE concept = ?1",
            params![concept.bytes().to_vec()],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    ActorId::from_bytes(blob16(r.get::<_, Vec<u8>>(2)?)),
                    r.get(3)?,
                ))
            },
        )
        .optional()
        .map_err(sql_err)
    }
    pub fn retracted_all(conn: &Connection) -> Result<Vec<(ConceptId, String)>> {
        let mut st = conn
            .prepare("SELECT concept, path FROM state_retracted")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|(a, p)| Some((ConceptId::from_slice(&a)?, p)))
            .collect();
        Ok(rows)
    }
    pub fn add_verified(
        conn: &Connection,
        concept: ConceptId,
        actor: ActorId,
        at: &str,
        content_version: ChangeId,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO state_verified (concept, actor, at, content_version) VALUES (?1, ?2, ?3, ?4)",
            params![concept.bytes().to_vec(), actor.bytes().to_vec(), at, content_version.bytes().to_vec()],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    /// Verifications at the current content version, oldest first.
    pub fn verified(
        conn: &Connection,
        concept: ConceptId,
        content_version: ChangeId,
    ) -> Result<Vec<(ActorId, String)>> {
        let mut st = conn
            .prepare("SELECT actor, at FROM state_verified WHERE concept = ?1 AND content_version = ?2 ORDER BY rowid")
            .map_err(sql_err)?;
        let rows = st
            .query_map(
                params![concept.bytes().to_vec(), content_version.bytes().to_vec()],
                |r| {
                    Ok((
                        ActorId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?)),
                        r.get::<_, String>(1)?,
                    ))
                },
            )
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
    pub fn add_erased_marker(
        conn: &Connection,
        concept: ConceptId,
        section: SectionId,
        after: Option<SectionId>,
        reason: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO state_erased_markers (concept, section, after_section, reason) VALUES (?1, ?2, ?3, ?4)",
            params![concept.bytes().to_vec(), section.bytes().to_vec(), after.map(|a| a.bytes().to_vec()), reason],
        )
        .map_err(sql_err)?;
        Ok(())
    }
    pub fn erased_markers(
        conn: &Connection,
        concept: ConceptId,
    ) -> Result<Vec<(SectionId, Option<SectionId>, String)>> {
        let mut st = conn
            .prepare("SELECT section, after_section, reason FROM state_erased_markers WHERE concept = ?1")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![concept.bytes().to_vec()], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Option<Vec<u8>>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|(s, a, reason)| {
                Some((
                    SectionId::from_slice(&s)?,
                    a.and_then(|b| SectionId::from_slice(&b)),
                    reason,
                ))
            })
            .collect();
        Ok(rows)
    }

    // ---- purge (§10.5 Phase 1) ----

    /// Section ids stored under `scope` (before purge).
    pub fn section_ids_in_scope(conn: &Connection, scope: ScopeId) -> Result<Vec<SectionId>> {
        let sb = scope.bytes().to_vec();
        let mut st = conn
            .prepare("SELECT section FROM state_sections WHERE scope = ?1")
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![sb], |r| r.get::<_, Vec<u8>>(0))
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|b| SectionId::from_slice(&b))
            .collect();
        Ok(rows)
    }

    /// Delete every row belonging to `scope` from the state tables. Returns affected concept IDs.
    pub fn purge_scope(conn: &Connection, scope: ScopeId) -> Result<Vec<ConceptId>> {
        let sb = scope.bytes().to_vec();
        let mut affected = Vec::new();
        // Sections in this scope.
        {
            let mut st = conn
                .prepare("SELECT concept, section FROM state_sections WHERE scope = ?1")
                .map_err(sql_err)?;
            let rows: Vec<(Vec<u8>, Vec<u8>)> = st
                .query_map(params![sb.clone()], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(sql_err)?
                .filter_map(|r| r.ok())
                .collect();
            for (c, s) in rows {
                if let Some(sid) = SectionId::from_slice(&s) {
                    crate::ri::unindex_section(conn, sid)?;
                }
                conn.execute(
                    "DELETE FROM state_links WHERE section = ?1",
                    params![s.clone()],
                )
                .map_err(sql_err)?;
                conn.execute(
                    "DELETE FROM section_dense_embeddings WHERE section = ?1",
                    params![s.clone()],
                )
                .map_err(sql_err)?;
                conn.execute("DELETE FROM state_sections WHERE section = ?1", params![s])
                    .map_err(sql_err)?;
                if let Some(c) = ConceptId::from_slice(&c) {
                    affected.push(c);
                }
            }
        }
        // Concepts in this scope.
        {
            let mut st = conn
                .prepare("SELECT concept FROM state_concepts WHERE scope = ?1")
                .map_err(sql_err)?;
            let rows: Vec<Vec<u8>> = st
                .query_map(params![sb.clone()], |r| r.get(0))
                .map_err(sql_err)?
                .filter_map(|r| r.ok())
                .collect();
            for c in rows {
                if let Some(cid) = ConceptId::from_slice(&c) {
                    Self::delete_concept(conn, cid)?;
                    conn.execute(
                        "DELETE FROM state_aliases WHERE concept = ?1",
                        params![c.clone()],
                    )
                    .map_err(sql_err)?;
                    conn.execute("DELETE FROM state_retracted WHERE concept = ?1", params![c])
                        .map_err(sql_err)?;
                    affected.push(cid);
                }
            }
        }
        conn.execute("DELETE FROM state_assets WHERE scope = ?1", params![sb])
            .map_err(sql_err)?;
        affected.sort();
        affected.dedup();
        Ok(affected)
    }

    pub fn vacuum(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA incremental_vacuum; PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(sql_err)?;
        Ok(())
    }

    // ---- search ----

    /// Tier 0: `#anchor` slug or `"quoted phrase"` substring match.
    pub fn search_exact(conn: &Connection, query: &str, k: usize) -> Result<Vec<SearchHit>> {
        let q = query.trim();
        let needle = if let Some(inner) = q.strip_prefix('#') {
            format!("#{}", inner.trim())
        } else if q.len() >= 2 && q.starts_with('"') && q.ends_with('"') {
            q[1..q.len() - 1].to_string()
        } else {
            return Ok(Vec::new());
        };
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let pat = format!("%{}%", needle.replace('%', "\\%"));
        let mut st = conn
            .prepare(
                "SELECT s.concept, s.section, c.path, s.heading, s.body
                 FROM state_sections s
                 JOIN state_concepts c ON c.concept = s.concept
                 WHERE s.level > 0 AND (s.heading LIKE ?1 ESCAPE '\\' OR s.body LIKE ?1 ESCAPE '\\')
                 ORDER BY s.idx LIMIT ?2",
            )
            .map_err(sql_err)?;
        Self::collect_search_rows(&mut st, params![pat, k as i64])
    }

    /// FTS5 trigram tokenizer (substring / hyphenation).
    pub fn search_substring_fts(
        conn: &Connection,
        query: &str,
        k: usize,
    ) -> Result<Vec<SearchHit>> {
        let q = fts_trigram_query(query);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let mut st = conn
            .prepare(
                "SELECT s.concept, s.section, c.path, s.heading, bm25(state_search_trigram) AS score, s.body
                 FROM state_search_trigram f
                 JOIN state_sections s ON s.rowid = f.rowid
                 JOIN state_concepts c ON c.concept = s.concept
                 WHERE state_search_trigram MATCH ?1
                 ORDER BY score LIMIT ?2",
            )
            .map_err(sql_err)?;
        Self::collect_search_rows_scored(&mut st, params![q, k as i64])
    }

    /// Character trigram Jaccard (typo-tolerant); merges with substring FTS candidates.
    pub fn search_char_trigram(
        conn: &Connection,
        query: &str,
        k: usize,
        min_score: Option<f32>,
    ) -> Result<Vec<SearchHit>> {
        let floor = min_score.unwrap_or(DEFAULT_CHAR_TRIGRAM_MIN);
        let q_tri = char_trigrams(query);
        if q_tri.is_empty() {
            return Ok(Vec::new());
        }
        let mut candidates: Vec<SearchHit> = Self::search_substring_fts(conn, query, k * 4)?;
        if candidates.is_empty() {
            candidates = Self::all_section_hits(conn)?;
        }
        let mut scored: Vec<SearchHit> = Vec::new();
        for mut h in candidates {
            let text = format!("{}\n{}", h.heading, h.body);
            let t_tri = char_trigrams(&text);
            let j = query_trigram_coverage(&q_tri, &t_tri);
            if j >= floor as f64 {
                h.score = j;
                scored.push(h);
            }
        }
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(k);
        Ok(scored)
    }

    fn all_section_hits(conn: &Connection) -> Result<Vec<SearchHit>> {
        let mut st = conn
            .prepare(
                "SELECT s.concept, s.section, c.path, s.heading, s.body
                 FROM state_sections s
                 JOIN state_concepts c ON c.concept = s.concept
                 WHERE s.level > 0",
            )
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| {
                Ok(SearchHit {
                    concept: ConceptId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?)),
                    section: SectionId::from_bytes(blob16(r.get::<_, Vec<u8>>(1)?)),
                    path: r.get(2)?,
                    heading: r.get(3)?,
                    score: 0.0,
                    body: r.get(4)?,
                })
            })
            .map_err(sql_err)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    fn collect_search_rows(
        st: &mut rusqlite::Statement,
        bind: impl rusqlite::Params,
    ) -> Result<Vec<SearchHit>> {
        let rows = st
            .query_map(bind, |r| {
                Ok(SearchHit {
                    concept: ConceptId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?)),
                    section: SectionId::from_bytes(blob16(r.get::<_, Vec<u8>>(1)?)),
                    path: r.get(2)?,
                    heading: r.get(3)?,
                    score: 1.0,
                    body: r.get(4)?,
                })
            })
            .map_err(sql_err)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    fn collect_search_rows_scored(
        st: &mut rusqlite::Statement,
        bind: impl rusqlite::Params,
    ) -> Result<Vec<SearchHit>> {
        let rows = st
            .query_map(bind, |r| {
                Ok(SearchHit {
                    concept: ConceptId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?)),
                    section: SectionId::from_bytes(blob16(r.get::<_, Vec<u8>>(1)?)),
                    path: r.get(2)?,
                    heading: r.get(3)?,
                    score: -r.get::<_, f64>(4)?,
                    body: r.get(5)?,
                })
            })
            .map_err(sql_err)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn dense_embedding_rows(conn: &Connection, section: SectionId) -> Result<u64> {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM section_dense_embeddings WHERE section = ?1",
                params![section.bytes().to_vec()],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        Ok(n as u64)
    }

    pub fn section_row_exists(conn: &Connection, section: SectionId) -> Result<bool> {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM state_sections WHERE section = ?1",
                params![section.bytes().to_vec()],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        Ok(n > 0)
    }

    /// Random-index cosine search (incremental TF-IDF vectors in hot.db).
    pub fn search_ri(
        conn: &Connection,
        query: &str,
        k: usize,
        min_score: f64,
        standalone_lexical_miss: bool,
    ) -> Result<Vec<SearchHit>> {
        crate::ri::search(conn, query, k, min_score, standalone_lexical_miss)
    }

    /// Keyword search with BM25 ranking over current sections.
    pub fn search_keyword(conn: &Connection, query: &str, k: usize) -> Result<Vec<SearchHit>> {
        let q = fts_query(query);
        let mut st = conn
            .prepare(
                "SELECT s.concept, s.section, c.path, s.heading, bm25(state_search_fts) AS score, s.body
                 FROM state_search_fts f JOIN state_sections s ON s.rowid = f.rowid JOIN state_concepts c ON c.concept = s.concept
                 WHERE state_search_fts MATCH ?1 ORDER BY score LIMIT ?2",
            )
            .map_err(sql_err)?;
        let rows = st
            .query_map(params![q, k as i64], |r| {
                Ok(SearchHit {
                    concept: ConceptId::from_bytes(blob16(r.get::<_, Vec<u8>>(0)?)),
                    section: SectionId::from_bytes(blob16(r.get::<_, Vec<u8>>(1)?)),
                    path: r.get(2)?,
                    heading: r.get(3)?,
                    score: -r.get::<_, f64>(4)?,
                    body: r.get(5)?,
                })
            })
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Does any plaintext in this hot tier contain `needle`? (Used by tests and `fsck`.)
    pub fn contains_text(conn: &Connection, needle: &str) -> Result<bool> {
        let pat = format!("%{needle}%");
        let n: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM state_sections WHERE body LIKE ?1 OR heading LIKE ?1)
                      + (SELECT COUNT(*) FROM state_frontmatter WHERE raw LIKE ?1)
                      + (SELECT COUNT(*) FROM state_sources WHERE record LIKE ?1)",
                params![pat],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        Ok(n > 0)
    }
}

/// Turn a free-text query into an FTS5 query: each token quoted, joined with implicit AND.
pub const DEFAULT_CHAR_TRIGRAM_MIN: f32 = 0.05;
/// When BM25 returns no hits, fuzzy leg must exceed this (avoids paraphrase ghost matches).
pub const STANDALONE_CHAR_TRIGRAM_MIN: f32 = 0.42;

fn char_trigrams(s: &str) -> HashSet<u32> {
    let flat: String = s
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    let mut set = HashSet::new();
    if flat.len() < 3 {
        return set;
    }
    let bytes = flat.as_bytes();
    for i in 0..bytes.len() - 2 {
        let tri = (bytes[i] as u32) << 16 | (bytes[i + 1] as u32) << 8 | bytes[i + 2] as u32;
        set.insert(tri);
    }
    set
}

fn query_trigram_coverage(query: &HashSet<u32>, doc: &HashSet<u32>) -> f64 {
    if query.is_empty() {
        return 0.0;
    }
    let inter = query.intersection(doc).count() as f64;
    inter / query.len() as f64
}

/// Longest token substring match for FTS5 trigram (hyphenation / partial terms).
pub fn fts_trigram_query(q: &str) -> String {
    let mut best = String::new();
    for tok in q.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_') {
        if tok.len() >= 3 && tok.len() > best.len() {
            best = tok.to_string();
        }
    }
    if best.is_empty() {
        return String::new();
    }
    format!("\"{}\"", best.replace('"', "\"\""))
}

pub fn fts_query(q: &str) -> String {
    let toks: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if toks.is_empty() {
        "\"\"".to_string()
    } else {
        toks.join(" ")
    }
}
