//! `wisp-sync`: the replica protocol (§11.6).
//!
//! Trusted local-path push/pull is implemented today; stdio framing is a skeleton for remote
//! transports. Session outline: Hello → Tombstones → Negotiate → Transfer → Refs.

mod http;
mod local;
mod pack;
mod protocol;
mod transport;

pub use http::serve_http;
pub use local::{init_trusted_replica, parse_local_url, pull, push};
pub use protocol::{client_hello_frames, server_ack_line, Message};
pub use transport::{parse_remote_url, pull_remote, push_remote, RemoteTarget};

pub const PROTOCOL_VERSION: u32 = 1;

use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Write};
use wisp_engine::repo::{Repository, Unlock};
use wisp_model::*;

/// Legacy entry point (returns not implemented for unknown remotes).
pub fn sync() -> Result<Value> {
    Err(WispError::not_implemented(
        "use push/pull with a configured remote or file:// path (M8)",
    ))
}

/// `wisp serve --stdio`: accept Hello + PushRef, then apply an in-process push from the client
/// bundle path sent on the third line (trusted local path transport).
pub fn serve_stdio(repo: &mut Repository, unlock: &Unlock) -> Result<Value> {
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let ref_name = protocol::server_handshake(repo, &mut reader)?;
    let mut path_line = String::new();
    reader
        .read_line(&mut path_line)
        .map_err(|e| WispError::internal(e.to_string()))?;
    let path = local::parse_local_url(path_line.trim())?;
    let mut client = Repository::open(&path, unlock)?;
    let result = local::push(&mut client, repo.root.as_path(), &ref_name, unlock)?;
    let mut stdout = io::stdout();
    protocol::write_message(
        &mut stdout,
        &Message::Done {
            result: result.clone(),
        },
    )?;
    stdout.flush()?;
    Ok(json!({ "serve": "stdio", "ref": ref_name, "push": result }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use wisp_engine::record::{record, RecordOptions};
    use wisp_engine::repo::MAIN_REF;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("bundle");
        std::fs::create_dir_all(&root).unwrap();
        std::env::remove_var("WISP_MASTER_KEY");
        (dir, root)
    }

    #[test]
    fn protocol_hello_golden() {
        let frames = client_hello_frames("01J0000000000000000000000", "main");
        assert_eq!(
            frames[0],
            include_str!("../tests/data/hello_golden.json").trim()
        );
        assert_eq!(frames[1], r#"{"type":"push_ref","ref":"main"}"#);
        assert_eq!(
            server_ack_line(),
            include_str!("../tests/data/ack_golden.json").trim()
        );
    }

    #[test]
    fn local_push_pull_roundtrip() {
        let (_dir, a_root) = fixture();
        let b_root = _dir.path().join("replica");
        std::fs::create_dir_all(&a_root).unwrap();
        std::fs::write(
            a_root.join("doc.md"),
            "---\ntype: Note\ntitle: Sync\n---\n\n## One\n\nbody\n",
        )
        .unwrap();
        let mut a = wisp_engine::Repository::init(&a_root, "0.2", &Unlock::default()).unwrap();
        record(
            &mut a,
            &RecordOptions {
                actor: Some("human:test".into()),
                message: Some("record".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let head_a = wisp_store::Ledger::head(a.ledger.conn(), MAIN_REF)
            .unwrap()
            .unwrap();
        let mut b = init_trusted_replica(&b_root, &a_root, &Unlock::default()).unwrap();
        let push = push(&mut a, &b_root, MAIN_REF, &Unlock::default()).unwrap();
        assert_eq!(push["changesets"].as_u64(), Some(1));
        let head_b = wisp_store::Ledger::head(b.ledger.conn(), MAIN_REF)
            .unwrap()
            .unwrap();
        assert_eq!(head_a, head_b);

        std::fs::write(
            a_root.join("doc.md"),
            "---\ntype: Note\ntitle: Sync\n---\n\n## One\n\nbody2\n",
        )
        .unwrap();
        record(
            &mut a,
            &RecordOptions {
                actor: Some("human:test".into()),
                message: Some("record2".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let head_a2 = wisp_store::Ledger::head(a.ledger.conn(), MAIN_REF)
            .unwrap()
            .unwrap();
        pull(&mut b, &a_root, MAIN_REF, &Unlock::default()).unwrap();
        let head_b2 = wisp_store::Ledger::head(b.ledger.conn(), MAIN_REF)
            .unwrap()
            .unwrap();
        assert_eq!(head_a2, head_b2);
    }

    #[test]
    fn server_handshake_reads_ref() {
        let (_dir, root) = fixture();
        let repo = wisp_engine::Repository::init(&root, "0.2", &Unlock::default()).unwrap();
        let id = repo.repo_id().to_string();
        let mut input = Cursor::new(format!(
            "{}\n{}\n",
            client_hello_frames(&id, "main")[0],
            client_hello_frames(&id, "main")[1],
        ));
        let r = protocol::server_handshake(&repo, &mut input).unwrap();
        assert_eq!(r, "main");
    }
}
