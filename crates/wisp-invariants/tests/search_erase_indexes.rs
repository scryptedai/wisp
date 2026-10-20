//! Erase must drop sections from BM25, trigram, and dense embedding tables.

use std::str::FromStr as _;
use wisp_engine::erase::{erase, EraseTarget};
use wisp_engine::query::search;
use wisp_engine::repo::{Unlock, MAIN_REF};
use wisp_invariants::{catalog, init_and_record_bundle};
use wisp_model::{Reason, SectionId};
use wisp_store::Hot;

#[test]
fn erase_purges_search_indexes() {
    let cat = catalog();
    let spec = cat.bundle("search-battery").expect("search-battery bundle");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let mut repo = init_and_record_bundle(spec, &root, &Unlock::default()).unwrap();

    let v = search(&mut repo, MAIN_REF, "out of memory", "auto", 5).unwrap();
    let hit = v["results"]
        .as_array()
        .unwrap()
        .first()
        .expect("OOM section searchable before erase");
    let path = hit["path"].as_str().unwrap();
    let section = hit["section"].as_str().unwrap();
    let section_id = SectionId::from_str(section).expect("section id");

    {
        let conn = repo.hot(MAIN_REF).unwrap().conn();
        conn.execute(
            "INSERT INTO section_dense_embeddings (section, model_tag, dimensions, vector)
             VALUES (?1, 'test-stub', 2, X'00000000000000000000000000000000')",
            rusqlite::params![section_id.bytes().to_vec()],
        )
        .unwrap();
        assert_eq!(Hot::dense_embedding_rows(conn, section_id).unwrap(), 1);
    }

    erase(
        &mut repo,
        MAIN_REF,
        &EraseTarget::Path(format!("{}#{}", path, section)),
        Reason::Policy,
        true,
        "human:test",
    )
    .unwrap();

    {
        let conn = repo.hot(MAIN_REF).unwrap().conn();
        assert!(!Hot::section_row_exists(conn, section_id).unwrap());
        assert_eq!(Hot::dense_embedding_rows(conn, section_id).unwrap(), 0);
        assert_eq!(wisp_store::ri::vector_rows(conn, section_id).unwrap(), 0);
        assert!(Hot::search_substring_fts(conn, "memory", 5)
            .unwrap()
            .iter()
            .all(|h| h.section != section_id));
    }
    assert!(
        search(&mut repo, MAIN_REF, "out of memory", "auto", 5).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
