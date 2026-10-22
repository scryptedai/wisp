//! Stdio replica push: JSON-lines Hello → PushRef → Ack → ColdPack → Done.

use crate::pack::{apply_pack_bytes, pack_bytes};
use crate::protocol::{read_message, write_message, Message};
use base64::Engine;
use std::io::{BufRead, Write};
use wisp_engine::apply;
use wisp_engine::repo::{Repository, Unlock};
use wisp_model::*;
use wisp_store::Ledger;

fn decode_cold_pack(msg: Message) -> Result<Vec<u8>> {
    match msg {
        Message::ColdPack { pack_b64 } => base64::engine::general_purpose::STANDARD
            .decode(pack_b64.trim())
            .map_err(|e| WispError::usage(format!("cold_pack base64: {e}"))),
        _ => Err(WispError::usage("expected ColdPack")),
    }
}

fn apply_cold_pack_push(
    repo: &mut Repository,
    unlock: &Unlock,
    ref_name: &str,
    bytes: &[u8],
) -> Result<serde_json::Value> {
    apply_pack_bytes(repo, bytes, unlock)?;
    if Ledger::head(repo.ledger.conn(), ref_name)?.is_some() {
        apply::rebuild_hot(repo, ref_name)?;
    }
    let head = Ledger::head(repo.ledger.conn(), ref_name)?;
    Ok(serde_json::json!({
        "direction": "push",
        "transport": "stdio",
        "ref": ref_name,
        "head": head.map(|h| h.to_string()),
        "bytes": bytes.len(),
    }))
}

/// Apply a `ColdPack` frame after handshake (server writes Ack before calling this).
pub fn apply_cold_pack_message(
    repo: &mut Repository,
    unlock: &Unlock,
    ref_name: &str,
    msg: Message,
    output: &mut impl Write,
) -> Result<serde_json::Value> {
    let bytes = decode_cold_pack(msg)?;
    let result = apply_cold_pack_push(repo, unlock, ref_name, &bytes)?;
    write_message(
        output,
        &Message::Done {
            result: result.clone(),
        },
    )?;
    output
        .flush()
        .map_err(|e| WispError::internal(e.to_string()))?;
    Ok(result)
}

/// Server: after [`protocol::server_handshake`], write Ack, read ColdPack, apply, respond Done.
pub fn serve_push_cold_pack(
    repo: &mut Repository,
    unlock: &Unlock,
    ref_name: &str,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<serde_json::Value> {
    write_message(
        output,
        &Message::Ack {
            protocol_version: crate::PROTOCOL_VERSION,
        },
    )?;
    output
        .flush()
        .map_err(|e| WispError::internal(e.to_string()))?;
    let msg = read_message(input)?.ok_or_else(|| WispError::usage("expected ColdPack"))?;
    apply_cold_pack_message(repo, unlock, ref_name, msg, output)
}

/// Client: Hello → PushRef → read Ack → ColdPack → read Done.
pub fn client_push_cold_pack(
    local: &Repository,
    ref_name: &str,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<serde_json::Value> {
    let hello = Message::Hello {
        repo_id: local.repo_id().to_string(),
        protocol_version: crate::PROTOCOL_VERSION,
        trusted: true,
        transport: "cold_pack".to_string(),
    };
    write_message(output, &hello)?;
    write_message(
        output,
        &Message::PushRef {
            r#ref: ref_name.to_string(),
        },
    )?;
    output
        .flush()
        .map_err(|e| WispError::internal(e.to_string()))?;
    let ack = read_message(input)?.ok_or_else(|| WispError::usage("expected Ack"))?;
    match ack {
        Message::Ack { protocol_version } if protocol_version == crate::PROTOCOL_VERSION => {}
        _ => return Err(WispError::usage("expected Ack")),
    }
    let body = pack_bytes(local)?;
    write_message(
        output,
        &Message::ColdPack {
            pack_b64: base64::engine::general_purpose::STANDARD.encode(&body),
        },
    )?;
    output
        .flush()
        .map_err(|e| WispError::internal(e.to_string()))?;
    let done = read_message(input)?.ok_or_else(|| WispError::usage("expected Done"))?;
    match done {
        Message::Done { result } => Ok(result),
        _ => Err(WispError::usage("expected Done")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wisp_engine::record::{record, RecordOptions};
    use wisp_engine::repo::{Repository, Unlock, MAIN_REF};
    use wisp_store::Ledger;

    #[test]
    fn cold_pack_apply_updates_replica_head() {
        let dir = tempfile::tempdir().unwrap();
        let a_root = dir.path().join("primary");
        let b_root = dir.path().join("replica");
        std::fs::create_dir_all(&a_root).unwrap();
        std::env::remove_var("WISP_MASTER_KEY");
        std::fs::write(
            a_root.join("doc.md"),
            "---\ntype: Note\ntitle: Pack\n---\n\n## One\n\nbody\n",
        )
        .unwrap();
        let mut a = Repository::init(&a_root, "0.2", &Unlock::default()).unwrap();
        record(
            &mut a,
            &RecordOptions {
                actor: Some("human:test".into()),
                message: Some("record".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let head_a = Ledger::head(a.ledger.conn(), MAIN_REF).unwrap().unwrap();
        drop(a);
        let unlock = Unlock::default();
        let mut b = Repository::init(&b_root, "0.2", &unlock).unwrap();
        let a = Repository::open(&a_root, &unlock).unwrap();
        let body = pack_bytes(&a).unwrap();
        apply_pack_bytes(&mut b, &body, &unlock).unwrap();
        if Ledger::head(b.ledger.conn(), MAIN_REF).unwrap().is_some() {
            apply::rebuild_hot(&mut b, MAIN_REF).unwrap();
        }
        let head_b = Ledger::head(b.ledger.conn(), MAIN_REF).unwrap().unwrap();
        assert_eq!(head_a, head_b);

        let decoded = decode_cold_pack(Message::ColdPack {
            pack_b64: base64::engine::general_purpose::STANDARD.encode(&body),
        })
        .unwrap();
        assert_eq!(decoded, body);
    }
}
