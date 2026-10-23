use crate::model::SCHEMA_VERSION;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use wisp_model::{Result, WispError};

/// Recompute content hashes from `manifest.json` and check `export-receipt.json`.
pub fn verify_site_dir(dir: &Path) -> Result<Value> {
    if !dir.is_dir() {
        return Err(WispError::usage(format!(
            "verify path is not a directory: {}",
            dir.display()
        )));
    }
    let manifest_path = dir.join("manifest.json");
    let receipt_path = dir.join("export-receipt.json");
    if !manifest_path.is_file() {
        return Err(WispError::usage("missing manifest.json"));
    }
    if !receipt_path.is_file() {
        return Err(WispError::usage("missing export-receipt.json"));
    }
    let manifest_text = fs::read_to_string(&manifest_path)?;
    let manifest: Value = serde_json::from_str(&manifest_text)
        .map_err(|e| WispError::integrity(format!("invalid manifest.json: {e}")))?;
    let schema = manifest
        .get("schema")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if schema != SCHEMA_VERSION {
        return Err(WispError::integrity(format!(
            "unsupported export schema {schema} (expected {SCHEMA_VERSION})"
        )));
    }
    let files = manifest
        .get("files")
        .and_then(|v| v.as_object())
        .ok_or_else(|| WispError::integrity("manifest missing files map"))?;

    let mut mismatches: Vec<String> = Vec::new();
    for (rel, expected) in files {
        if rel == "manifest.json" {
            continue;
        }
        let expected = expected
            .as_str()
            .ok_or_else(|| WispError::integrity("manifest file hash not a string"))?;
        let path = dir.join(rel);
        if !path.is_file() {
            mismatches.push(format!("missing file: {rel}"));
            continue;
        }
        let bytes = fs::read(&path)?;
        let got = sha256_hex(&bytes);
        if got != expected {
            mismatches.push(format!("hash mismatch: {rel}"));
        }
    }

    let manifest_hash = sha256_hex(manifest_text.as_bytes());
    let receipt: Value = serde_json::from_str(&fs::read_to_string(&receipt_path)?)
        .map_err(|e| WispError::integrity(format!("invalid export-receipt.json: {e}")))?;
    let receipt_manifest = receipt
        .get("manifest_sha256")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if receipt_manifest != manifest_hash {
        mismatches.push("receipt manifest_sha256 does not match manifest.json".to_string());
    }

    let signed = receipt.get("signature").is_some();
    Ok(serde_json::json!({
        "ok": mismatches.is_empty(),
        "command": "export",
        "data": {
            "kind": "html_verify",
            "dir": dir.display().to_string(),
            "schema": schema,
            "file_count": files.len(),
            "mismatches": mismatches,
            "manifest_sha256": manifest_hash,
            "signed": signed,
        }
    }))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let h = Sha256::digest(bytes);
    hex::encode(h)
}
