//! Cold-tier pack as `.tar.zst` (git-style dumb transport payload).

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder};
use wisp_engine::repo::{Repository, Unlock};
use wisp_model::*;
use zstd::stream::{read::Decoder, write::Encoder};

const PACK_ROOT: &str = "wisp-cold";

/// Write `.wisp/` cold tier (ledger db, payloads, keys) to a zstd tar.
pub fn write_cold_pack(repo: &Repository, dest: &Path) -> Result<()> {
    let wisp = &repo.wisp_dir;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(dest)?;
    let enc = Encoder::new(file, 3)?;
    let mut tar = Builder::new(enc);
    for rel in cold_paths(wisp)? {
        let abs = wisp.join(&rel);
        if abs.is_file() {
            tar.append_path_with_name(&abs, format!("{}/{}", PACK_ROOT, rel.display()))
                .map_err(|e| WispError::internal(e.to_string()))?;
        }
    }
    tar.finish()
        .map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

pub fn read_cold_pack(repo: &mut Repository, pack: &Path) -> Result<()> {
    let file = File::open(pack)?;
    let dec = Decoder::new(file)?;
    let mut archive = Archive::new(dec);
    let extract_to = repo.wisp_dir.parent().unwrap();
    for entry in archive
        .entries()
        .map_err(|e| WispError::internal(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| WispError::internal(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| WispError::internal(e.to_string()))?
            .to_path_buf();
        if !path.starts_with(PACK_ROOT) {
            continue;
        }
        let rel = path.strip_prefix(PACK_ROOT).unwrap();
        let out = extract_to.join(".wisp").join(rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if entry.header().entry_type().is_file() {
            let mut f = File::create(&out)?;
            std::io::copy(&mut entry, &mut f)?;
        }
    }
    Ok(())
}

pub fn pack_bytes(repo: &Repository) -> Result<Vec<u8>> {
    let tmp = tempfile::NamedTempFile::new()?;
    write_cold_pack(repo, tmp.path())?;
    let mut f = File::open(tmp.path())?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

pub fn apply_pack_bytes(repo: &mut Repository, bytes: &[u8], unlock: &Unlock) -> Result<()> {
    let tmp = tempfile::NamedTempFile::new()?;
    tmp.as_file().write_all(bytes)?;
    read_cold_pack(repo, tmp.path())?;
    let root = repo.root.clone();
    *repo = Repository::open(&root, unlock)?;
    Ok(())
}

fn cold_paths(wisp: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for rel in ["ledger.db", "ledger.db-wal", "ledger.db-shm"] {
        if wisp.join(rel).is_file() {
            out.push(PathBuf::from(rel));
        }
    }
    walk_files(&wisp.join("payloads"), &mut out, Path::new("payloads"))?;
    walk_files(&wisp.join("keys"), &mut out, Path::new("keys"))?;
    Ok(out)
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>, prefix: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        let rel = prefix.join(e.file_name());
        if p.is_dir() {
            walk_files(&p, out, &rel)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}
