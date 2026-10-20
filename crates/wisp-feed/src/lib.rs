//! `wisp-feed`: change feed, cursors, and the subscriber host (§14).
//!
//! The feed query (`wisp feed --since <seq>`) and cursors are implemented in `wisp-engine`
//! (`query::feed`, `query::index`). The external subscriber host — spawning `command` subscribers,
//! delivering JSON lines on stdin, advancing cursors on `{"ack": seq}` — is milestone M5.

use wisp_model::{Result, WispError};

/// The subscriber contract (§14.2).
pub const CONTRACT: &str = "events on stdin as JSON lines; acknowledgements {\"ack\": <seq>} on stdout; at-least-once, in order; idempotent on (change, version); an erase event is acknowledged only after every derived artifact for the listed targets is deleted and compacted";

pub fn run_external_subscribers() -> Result<serde_json::Value> {
    Err(WispError::not_implemented("external subscriber host (M5)"))
}
