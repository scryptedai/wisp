//! `wisp-store`: the cold-tier ledger, the hot-tier cache, payload files and refs (§6).

pub mod hot;
pub mod ledger;
pub mod ri;

pub use hot::{
    AssetState, ConceptState, FrontmatterState, Hot, HotMeta, SearchHit, SectionState, SourceState,
    DEFAULT_CHAR_TRIGRAM_MIN, STANDALONE_CHAR_TRIGRAM_MIN,
};
pub use ledger::{ActorRow, ChangesetRow, Ledger, ScopeRow, Tombstone};
pub use ri::{RI_DIM, RI_MIN_COSINE, RI_STANDALONE_MIN_COSINE};

use wisp_model::WispError;

pub type SqlResult<T> = std::result::Result<T, WispError>;

pub(crate) fn sql_err(e: rusqlite::Error) -> WispError {
    match e {
        rusqlite::Error::SqliteFailure(f, msg) => {
            if f.code == rusqlite::ErrorCode::DatabaseBusy
                || f.code == rusqlite::ErrorCode::DatabaseLocked
            {
                WispError::timeout("write queue timeout")
            } else if msg
                .as_deref()
                .map(|m| m.contains("append-only"))
                .unwrap_or(false)
            {
                WispError::integrity("ledger is append-only")
            } else {
                WispError::internal(format!("sqlite: {} {}", f, msg.unwrap_or_default()))
            }
        }
        e => WispError::internal(format!("sqlite: {e}")),
    }
}

pub(crate) fn blob16(b: Vec<u8>) -> [u8; 16] {
    let mut a = [0u8; 16];
    if b.len() == 16 {
        a.copy_from_slice(&b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use wisp_model::*;

    fn sample_change(cs: ChangesetId, ord: u32) -> Change {
        Change {
            id: ChangeId::new(),
            changeset: cs,
            ordinal: ord,
            op: Op::CreateConcept,
            target: Target::Concept {
                concept: ConceptId::new(),
            },
            scope: ScopeId::new(),
            payload: None,
            note: None,
            inputs: vec![],
            valid: None,
            lineage: None,
            origin: None,
            reason: None,
        }
    }

    #[test]
    fn i1_ledger_is_append_only() {
        let dir = tempfile::tempdir().unwrap();
        let wisp = dir.path().join(".wisp");
        std::fs::create_dir_all(&wisp).unwrap();
        let mut l = Ledger::open(&wisp, 1000).unwrap();
        let cs = Changeset {
            id: ChangesetId::new(),
            parents: vec![],
            based_on: None,
            actor_id: ActorId::new(),
            at: now_rfc3339(),
            message: None,
            digest: [0u8; 32],
            sig: None,
        };
        let ch = vec![sample_change(cs.id, 0)];
        {
            let tx = l.begin_immediate().unwrap();
            Ledger::insert_changeset(&tx, &cs, &ch).unwrap();
            tx.commit().unwrap();
        }
        let got = Ledger::get_changeset(l.conn(), cs.id).unwrap().unwrap();
        assert_eq!(got.seq, 1);
        assert_eq!(Ledger::changes_of(l.conn(), cs.id).unwrap().len(), 1);
        // UPDATE and DELETE are rejected by triggers.
        let upd = l.conn().execute("UPDATE changesets SET at = 'x'", []);
        assert!(upd.is_err());
        let del = l.conn().execute("DELETE FROM changes", []);
        assert!(del.is_err());
        // Feed order.
        let feed = Ledger::changes_since(l.conn(), 0).unwrap();
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].0, 1);
    }

    #[test]
    fn staged_payloads_flush_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let wisp = dir.path().join(".wisp");
        std::fs::create_dir_all(&wisp).unwrap();
        let l = Ledger::open(&wisp, 1000).unwrap();
        let id = PayloadId::new();
        Ledger::stage_payload(l.conn(), id, b"cipher").unwrap();
        // Readable while staged.
        assert_eq!(l.read_payload(id).unwrap().unwrap(), b"cipher");
        assert_eq!(l.flush_staged().unwrap(), 1);
        assert!(l.payloads_dir().join(Ledger::payload_rel_path(id)).exists());
        assert_eq!(l.read_payload(id).unwrap().unwrap(), b"cipher");
        // Idempotent.
        assert_eq!(l.flush_staged().unwrap(), 0);
    }

    #[test]
    fn hot_fts_and_purge() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Hot::open(&dir.path().join("hot.db")).unwrap();
        let c = ConceptId::new();
        let scope = ScopeId::new();
        let sscope = ScopeId::new();
        let v = ChangeId::new();
        {
            let tx = h.begin().unwrap();
            Hot::upsert_concept(
                &tx,
                &ConceptState {
                    concept: c,
                    path: "tables/orders".into(),
                    type_: Some("Table".into()),
                    status: None,
                    scope,
                    version: v,
                    gen_actor: None,
                    gen_at: None,
                    content_version: Some(v),
                },
            )
            .unwrap();
            let s = SectionId::new();
            Hot::upsert_section(
                &tx,
                &SectionState {
                    concept: c,
                    section: s,
                    idx: 1,
                    level: 2,
                    heading: "Schema".into(),
                    heading_line: "## Schema\n".into(),
                    body: "The canary CANARY_ONE lives here.\n".into(),
                    scope: sscope,
                    version: v,
                },
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let hits = Hot::search_keyword(h.conn(), "canary", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(Hot::contains_text(h.conn(), "CANARY_ONE").unwrap());
        let affected = Hot::purge_scope(h.conn(), sscope).unwrap();
        assert_eq!(affected, vec![c]);
        assert!(Hot::search_keyword(h.conn(), "canary", 10)
            .unwrap()
            .is_empty());
        assert!(!Hot::contains_text(h.conn(), "CANARY_ONE").unwrap());
    }

    #[test]
    fn fts_query_quoting() {
        assert_eq!(hot::fts_query("door behavior"), "\"door\" \"behavior\"");
        assert_eq!(hot::fts_query("a-b \"x\""), "\"a-b\" \"x\"");
    }
}
