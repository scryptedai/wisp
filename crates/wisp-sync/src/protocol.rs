//! JSON-lines replica session framing (§11.6 skeleton).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};
use wisp_engine::repo::Repository;
use wisp_model::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        repo_id: String,
        protocol_version: u32,
        trusted: bool,
    },
    Ack {
        protocol_version: u32,
    },
    PushRef {
        r#ref: String,
    },
    Done {
        result: Value,
    },
}

pub fn write_message(w: &mut impl Write, msg: &Message) -> Result<()> {
    let line = serde_json::to_string(msg)?;
    w.write_all(line.as_bytes())?;
    w.write_all(b"\n")?;
    Ok(())
}

pub fn read_message(r: &mut impl BufRead) -> Result<Option<Message>> {
    let mut line = String::new();
    let n = r.read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(line).map_err(|e| WispError::usage(format!("bad sync frame: {e}")))
}

/// Server handshake: Hello → Ack → PushRef (returns ref name).
pub fn server_handshake(repo: &Repository, input: &mut impl BufRead) -> Result<String> {
    let hello = read_message(input)?.ok_or_else(|| WispError::usage("expected Hello"))?;
    match hello {
        Message::Hello {
            repo_id,
            protocol_version,
            trusted,
        } => {
            if protocol_version != crate::PROTOCOL_VERSION {
                return Err(WispError::usage(format!(
                    "protocol version {protocol_version} != {}",
                    crate::PROTOCOL_VERSION
                )));
            }
            if repo_id != repo.repo_id().to_string() {
                return Err(WispError::usage("repo_id mismatch in Hello"));
            }
            if !trusted {
                return Err(WispError::usage("stdio serve requires trusted Hello"));
            }
        }
        _ => return Err(WispError::usage("expected Hello")),
    }
    let push = read_message(input)?.ok_or_else(|| WispError::usage("expected PushRef"))?;
    match push {
        Message::PushRef { r#ref } => Ok(r#ref),
        _ => Err(WispError::usage("expected PushRef")),
    }
}

/// Client handshake bytes for golden tests.
pub fn client_hello_frames(repo_id: &str, r#ref: &str) -> Vec<String> {
    let hello = Message::Hello {
        repo_id: repo_id.to_string(),
        protocol_version: crate::PROTOCOL_VERSION,
        trusted: true,
    };
    let push = Message::PushRef {
        r#ref: r#ref.to_string(),
    };
    vec![
        serde_json::to_string(&hello).unwrap(),
        serde_json::to_string(&push).unwrap(),
    ]
}

pub fn server_ack_line() -> String {
    serde_json::to_string(&Message::Ack {
        protocol_version: crate::PROTOCOL_VERSION,
    })
    .unwrap()
}
