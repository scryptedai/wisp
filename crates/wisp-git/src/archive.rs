//! `git archive`-style bundle export as `.tar.zst`.

use serde_json::{json, Value};
use std::fs::File;
use std::path::Path;
use tar::Builder;
use wisp_engine::checkout;
use wisp_engine::repo::Repository;
use wisp_model::*;
use zstd::stream::write::Encoder;

/// Render `ref_name` and write a zstd-compressed tar at `archive_path`.
pub fn export_archive(
    repo: &mut Repository,
    ref_name: &str,
    archive_path: &Path,
    profile: Option<&str>,
    strip_anchors: bool,
) -> Result<Value> {
    let tmp = tempfile::tempdir().map_err(|e| WispError::internal(e.to_string()))?;
    let idx = checkout::checkout_with_profile(
        repo,
        ref_name,
        Some(tmp.path()),
        profile,
        strip_anchors,
        false,
    )?;
    if let Some(parent) = archive_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(archive_path)?;
    let enc = Encoder::new(file, 3)?;
    let mut tar = Builder::new(enc);
    tar.append_dir_all(".", tmp.path())
        .map_err(|e| WispError::internal(e.to_string()))?;
    tar.finish()
        .map_err(|e| WispError::internal(e.to_string()))?;
    Ok(json!({
        "archive": archive_path.display().to_string(),
        "ref": ref_name,
        "profile": profile,
        "files": idx.files.len(),
        "format": "tar.zst",
    }))
}
