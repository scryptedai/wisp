//! Refresh-on-conflict helper for headless `apply` (exit 3).

use crate::headless::{apply, ApplyBatch, ApplyOp};
use crate::repo::Repository;
use serde_json::Value;
use std::thread;
use std::time::Duration;
use wisp_model::{ChangeId, ExitCode, Result, SectionId, WispError};
use wisp_store::Hot;

/// Policy for [`apply_with_resolve`].
#[derive(Clone, Debug)]
/// Retry budget for [`apply_with_resolve`] (`wisp apply --resolve`).
pub struct ApplyResolvePolicy {
    /// Total apply attempts (first try counts).
    pub max_attempts: u32,
    /// Base sleep after exit 3 (ms); jittered in the loop. `from_manifest` sets a small default.
    pub conflict_sleep_ms: u64,
    /// Sleep after a retryable (exit 7) failure once commit retries are exhausted (ms).
    pub retryable_sleep_ms: u64,
}

impl Default for ApplyResolvePolicy {
    fn default() -> Self {
        Self {
            max_attempts: 24,
            conflict_sleep_ms: 0,
            retryable_sleep_ms: 0,
        }
    }
}

impl ApplyResolvePolicy {
    /// Scale attempt budget from manifest `commit.retries`.
    pub fn from_manifest(commit_retries: u32) -> Self {
        let r = commit_retries.max(1);
        Self {
            max_attempts: (r * 8).max(12),
            conflict_sleep_ms: 5,
            ..Default::default()
        }
    }
}

/// Counters from a successful or failed resolve loop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyResolveStats {
    pub attempts: u32,
    pub conflict_rounds: u32,
    pub retryable_errors: u32,
}

/// Run `apply`, refreshing `base_version` on **section_conflict** until success or the policy limit.
pub fn apply_with_resolve(
    repo: &mut Repository,
    mut batch: ApplyBatch,
    default_ref: Option<&str>,
    policy: &ApplyResolvePolicy,
) -> Result<(Value, ApplyResolveStats)> {
    let ref_name = batch
        .r#ref
        .clone()
        .or_else(|| default_ref.map(|s| s.to_string()))
        .unwrap_or_else(|| crate::repo::MAIN_REF.to_string());
    let max = policy.max_attempts.max(1);
    let mut stats = ApplyResolveStats::default();
    let mut last_err: Option<WispError> = None;

    for attempt in 1..=max {
        stats.attempts = attempt;
        match apply(repo, batch.clone(), default_ref) {
            Ok(v) => return Ok((v, stats)),
            Err(e) if e.code == ExitCode::Conflict => {
                stats.conflict_rounds += 1;
                let conflicts = e.details.get("conflicts").cloned().unwrap_or(Value::Null);
                if !patch_batch_from_conflicts(repo, &ref_name, &mut batch, &conflicts)? {
                    return Err(e);
                }
                last_err = Some(e);
                if policy.conflict_sleep_ms > 0 {
                    let jitter_ms =
                        rand::random::<u64>() % policy.conflict_sleep_ms.saturating_add(1);
                    thread::sleep(Duration::from_millis(
                        policy.conflict_sleep_ms.saturating_add(jitter_ms),
                    ));
                }
            }
            Err(e) if e.is_retryable() => {
                stats.retryable_errors += 1;
                refresh_batch_sections_from_hot(repo, &ref_name, &mut batch)?;
                last_err = Some(e);
                if policy.retryable_sleep_ms > 0 {
                    thread::sleep(Duration::from_millis(policy.retryable_sleep_ms));
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        WispError::conflict(
            format!("apply_with_resolve exceeded {max} attempts"),
            Value::Null,
        )
    }))
}

/// Patch `base_version` on batch ops from conflict JSON (`details.conflicts`). Returns false when
/// conflicts are not auto-resolvable (e.g. `section_removed`).
pub fn patch_batch_from_conflicts(
    repo: &mut Repository,
    ref_name: &str,
    batch: &mut ApplyBatch,
    conflicts: &Value,
) -> Result<bool> {
    let arr = conflicts
        .as_array()
        .ok_or_else(|| WispError::internal("conflict details missing array"))?;
    if arr.is_empty() {
        return Ok(false);
    }
    let mut patched = false;
    for c in arr {
        let typ = c.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if typ != "section_conflict" {
            return Ok(false);
        }
        let section = c
            .get("section")
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<SectionId>().ok());
        let theirs = c
            .get("theirs")
            .and_then(|t| t.get("version"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<ChangeId>().ok());
        let section = match section {
            Some(s) => s,
            None => return Ok(false),
        };
        let version = match theirs {
            Some(v) => v,
            None => refresh_section_version(repo, ref_name, batch, section)?,
        };
        if patch_ops_for_section(&mut batch.ops, section, version) {
            patched = true;
        }
    }
    if !patched {
        refresh_batch_sections_from_hot(repo, ref_name, batch)?;
        return Ok(batch_has_versioned_sections(batch));
    }
    Ok(true)
}

fn batch_has_versioned_sections(batch: &ApplyBatch) -> bool {
    batch.ops.iter().any(op_wants_section_base)
}

fn op_wants_section_base(op: &ApplyOp) -> bool {
    if op.section.is_none() {
        return false;
    }
    let op_name = op.op.as_str();
    if op_name == "append_section" || op_name == "prepend_section" {
        return false;
    }
    !matches!(op.base_version.as_deref(), Some("any"))
}

fn patch_ops_for_section(ops: &mut [ApplyOp], section: SectionId, version: ChangeId) -> bool {
    let ver = version.to_string();
    let mut any = false;
    for op in ops.iter_mut() {
        if op.section != Some(section) || !op_wants_section_base(op) {
            continue;
        }
        op.base_version = Some(ver.clone());
        any = true;
    }
    any
}

fn refresh_section_version(
    repo: &mut Repository,
    ref_name: &str,
    batch: &mut ApplyBatch,
    section: SectionId,
) -> Result<ChangeId> {
    refresh_batch_sections_from_hot(repo, ref_name, batch)?;
    let conn = repo.hot(ref_name)?.conn();
    let st = Hot::section(conn, section)?
        .ok_or_else(|| WispError::not_found("section removed during resolve"))?;
    Ok(st.version)
}

fn refresh_batch_sections_from_hot(
    repo: &mut Repository,
    ref_name: &str,
    batch: &mut ApplyBatch,
) -> Result<()> {
    let conn = repo.hot(ref_name)?.conn();
    for op in &mut batch.ops {
        if !op_wants_section_base(op) {
            continue;
        }
        let section = op.section.unwrap();
        if let Some(st) = Hot::section(conn, section)? {
            op.base_version = Some(st.version.to_string());
        }
    }
    Ok(())
}
