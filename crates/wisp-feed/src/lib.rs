//! `wisp-feed`: change feed, cursors, and the subscriber host (§14).
//!
//! The feed query (`wisp feed --since <seq>`) and cursors are implemented in `wisp-engine`
//! (`query::feed`, `query::index`). This crate hosts external `command` subscribers.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::str::FromStr;
use wisp_engine::query;
use wisp_engine::Repository;
use wisp_model::{ChangeId, Result, WispError};
use wisp_store::Ledger;

/// The subscriber contract (§14.2).
pub const CONTRACT: &str = "events on stdin as JSON lines; acknowledgements {\"ack\": <seq>} on stdout; at-least-once, in order; idempotent on (change, version); an erase event is acknowledged only after every derived artifact for the listed targets is deleted and compacted";

/// Spawn each manifest `command` subscriber, deliver feed events since its cursor, advance on `{"ack": seq}`.
pub fn run_external_subscribers(repo: &mut Repository) -> Result<Value> {
    let mut ran = Vec::new();
    for sub in repo.manifest.subscribers.clone() {
        if sub.builtin.is_some() {
            continue;
        }
        let cmd = match &sub.command {
            Some(c) if !c.is_empty() => c,
            _ => {
                ran.push(json!({
                    "subscriber": sub.name,
                    "skipped": true,
                    "reason": "no command configured",
                }));
                continue;
            }
        };
        let cursor_before = Ledger::cursor(repo.ledger.conn(), &sub.name)?;
        let events = query::feed(repo, cursor_before)?;
        if events.is_empty() {
            ran.push(json!({
                "subscriber": sub.name,
                "cursor": cursor_before,
                "delivered": 0,
            }));
            continue;
        }
        let mut child = Command::new(&cmd[0])
            .args(&cmd[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| WispError::internal(format!("spawn {}: {e}", sub.name)))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| WispError::internal("subscriber stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| WispError::internal("subscriber stdout"))?;
        let mut reader = BufReader::new(stdout);
        let mut delivered = 0usize;
        let mut cursor = cursor_before;
        for ev in &events {
            let line = serde_json::to_string(ev)?;
            stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .map_err(|e| WispError::internal(format!("write to {}: {e}", sub.name)))?;
            stdin
                .flush()
                .map_err(|e| WispError::internal(format!("flush {}: {e}", sub.name)))?;
            let mut ack_line = String::new();
            reader
                .read_line(&mut ack_line)
                .map_err(|e| WispError::internal(format!("read ack from {}: {e}", sub.name)))?;
            let ack: Value = serde_json::from_str(ack_line.trim()).map_err(|e| {
                WispError::internal(format!(
                    "invalid ack from {}: {} ({})",
                    sub.name, e, ack_line
                ))
            })?;
            let ev_seq = ev
                .get("seq")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| WispError::internal("feed event missing seq".to_string()))?;
            let seq = ack
                .get("ack")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| WispError::internal(format!("ack missing seq from {}", sub.name)))?;
            if seq != ev_seq {
                return Err(WispError::internal(format!(
                    "subscriber {} ack seq {seq} does not match event seq {ev_seq}",
                    sub.name
                )));
            }
            Ledger::set_cursor(repo.ledger.conn(), &sub.name, seq)?;
            cursor = seq;
            delivered += 1;
            if ev.get("event").and_then(|v| v.as_str()) == Some("erase") {
                if let Some(ch) = ev.get("change").and_then(|v| v.as_str()) {
                    if let Ok(id) = ChangeId::from_str(ch) {
                        Ledger::clear_pending_erasure(repo.ledger.conn(), id, &sub.name)?;
                    }
                }
            }
        }
        drop(stdin);
        let status = child
            .wait()
            .map_err(|e| WispError::internal(format!("wait {}: {e}", sub.name)))?;
        if !status.success() {
            return Err(WispError::internal(format!(
                "subscriber {} exited with {}",
                sub.name, status
            )));
        }
        ran.push(json!({
            "subscriber": sub.name,
            "cursor": cursor,
            "delivered": delivered,
            "external": true,
        }));
    }
    repo.ledger.checkpoint()?;
    Ok(json!({ "subscribers": ran }))
}
