//! `wisp-engine`: orchestrates record, checkout, erase, headless writes, and queries; owns transactions.

pub mod apply;
pub mod apply_resolve;
pub mod cache;
pub mod checkout;
pub mod cold_flush;
pub mod commit;
pub mod erase;
pub mod headless;
pub mod html_export;
pub mod impact;
pub mod manifest;
pub mod materialize;
pub mod merge;
pub mod payloads;
pub mod publish;
pub mod query;
pub mod record;
pub mod repo;
pub mod search;
pub mod signing;
pub mod snapshot;
pub mod timing;
pub mod vector;
pub mod worktree;

pub use repo::{Repository, Unlock, MAIN_REF};

/// Parse a YAML document into JSON (for `--frontmatter-file`).
pub fn yaml_to_json(s: &str) -> wisp_model::Result<serde_json::Value> {
    let v: serde_yaml::Value =
        serde_yaml::from_str(s).map_err(|e| wisp_model::WispError::usage(format!("yaml: {e}")))?;
    serde_json::to_value(v).map_err(|e| wisp_model::WispError::usage(format!("yaml: {e}")))
}

#[cfg(test)]
mod tests;
