//! `wisp-merge`: three-way merge over changesets (§11.5).
//!
//! Not implemented in this build (milestone M7). The merge rules table from §11.5 is reproduced in
//! `RULES` so the implementation and its tests can be written against it.

use wisp_model::{Result, WispError};

/// §11.5 merge rules: (both sides changed, result).
pub const RULES: &[(&str, &str)] = &[
    ("same section, same final commitment", "no conflict"),
    ("same section, different final content", "conflict"),
    (
        "section removed on one side, updated on the other",
        "conflict",
    ),
    ("same frontmatter entry, same parsed value", "no conflict"),
    ("same frontmatter entry, different values", "conflict"),
    ("verify events", "union, no conflict"),
    (
        "section order",
        "take ours, then insert theirs' new sections after their nearest preceding matched section",
    ),
    ("different paths for the same concept", "conflict"),
    ("two concepts at the same path", "conflict"),
    (
        "retract on one side, content change on the other",
        "conflict",
    ),
    ("set_status both sides, different values", "conflict"),
    (
        "erase on either side",
        "erase wins; the other side's changes to that scope are dropped and reported",
    ),
];

pub fn merge() -> Result<serde_json::Value> {
    Err(WispError::not_implemented("merge (M7)"))
}
