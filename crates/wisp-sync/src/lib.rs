//! `wisp-sync`: the replica protocol (§11.6). Not implemented in this build (milestone M8).
//!
//! Session outline, for the implementation: Hello (RepoId, protocol version, trusted flag) →
//! Tombstones (each side sends tombstones the other lacks; receiver applies erases first) →
//! Negotiate (heads and known changeset IDs since the last sync point) → Transfer (changesets in
//! topological order, payloads for scopes not erased on the receiver, wrapped keys for trusted
//! peers re-wrapped under an X25519 session key) → Refs (push fast-forwards only; pull merges).

use wisp_model::{Result, WispError};

pub const PROTOCOL_VERSION: u32 = 1;

pub fn sync() -> Result<serde_json::Value> {
    Err(WispError::not_implemented("remote/push/pull/serve (M8)"))
}
