//! `wisp-git`: git import and export as transports (§13.2, §13.3). Not implemented in this build (M8).
//!
//! `wisp import <dir>` (a plain bundle) and `wisp export --archive` (a rendered directory) are
//! available through the engine today; `--git` import with history and per-changeset git commits
//! on export will build on `gix`.

use wisp_model::{Result, WispError};

pub fn import_git() -> Result<serde_json::Value> {
    Err(WispError::not_implemented(
        "import --git (M8); use `wisp import <dir>` for a checked-out bundle",
    ))
}

pub fn export_git() -> Result<serde_json::Value> {
    Err(WispError::not_implemented(
        "export --git (M8); use `wisp export --archive <dir>` for a rendered bundle",
    ))
}
