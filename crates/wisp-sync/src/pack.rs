//! Cold-tier pack as `.tar.zst` (git-style dumb transport payload).

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use tar::{Archive, Builder};
use wisp_engine::repo::{Repository, Unlock};
use wisp_model::*;
use zstd::stream::{read::Decoder, write::Encoder};

const PACK_ROOT: &str = "wisp-cold";

/// When unset or not `1`, packs omit `keys/` (scope keys stay local; use `file://` sync for keys).
pub fn pack_includes_keys() -> bool {
    std::env::var("WISP_PACK_INCLUDE_KEYS").ok().as_deref() == Some("1")
}

/// Write `.wisp/` cold tier (ledger db, payloads; keys only when `WISP_PACK_INCLUDE_KEYS=1`).
pub fn write_cold_pack(repo: &mut Repository, dest: &Path) -> Result<()> {
    repo.flush_cold_payloads()?;
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
    let enc = tar
        .into_inner()
        .map_err(|e| WispError::internal(e.to_string()))?;
    enc.finish()
        .map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

pub fn read_cold_pack_into(wisp_dir: &Path, pack: &Path) -> Result<()> {
    let file = File::open(pack)?;
    let dec = Decoder::new(file)?;
    let mut archive = Archive::new(dec);
    let extract_root = wisp_dir;
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
        let rel = path
            .strip_prefix(PACK_ROOT)
            .map_err(|e| WispError::usage(format!("pack entry: {e}")))?;
        if !pack_includes_keys() && rel.starts_with("keys") {
            continue;
        }
        let out = safe_wisp_path(extract_root, rel)?;
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

/// Reject `..` and absolute path components; destination must stay under `wisp_dir`.
pub fn safe_wisp_path(wisp_dir: &Path, rel: &Path) -> Result<PathBuf> {
    for c in rel.components() {
        match c {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(WispError::usage(format!(
                    "illegal pack member path {}",
                    rel.display()
                )));
            }
            _ => {}
        }
    }
    let out = wisp_dir.join(rel);
    Ok(out)
}

pub fn pack_bytes(repo: &mut Repository) -> Result<Vec<u8>> {
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
    let pack = tmp.path();
    let (root, wisp_dir) = Repository::relinquish_for_cold_overwrite(repo);
    if let Err(e) = read_cold_pack_into(&wisp_dir, pack) {
        let opened = Repository::open(&root, unlock)?;
        unsafe {
            std::ptr::write(repo, opened);
        }
        return Err(e);
    }
    let opened = Repository::open(&root, unlock)?;
    unsafe {
        std::ptr::write(repo, opened);
    }
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
    if pack_includes_keys() {
        walk_files(&wisp.join("keys"), &mut out, Path::new("keys"))?;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use wisp_engine::repo::{Repository, Unlock};

    #[test]
    fn safe_wisp_path_rejects_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let wisp = dir.path().join(".wisp");
        std::fs::create_dir_all(&wisp).unwrap();
        assert!(safe_wisp_path(&wisp, Path::new("../ledger.db")).is_err());
        assert!(safe_wisp_path(&wisp, Path::new("payloads/ab/x")).is_ok());
    }

    #[test]
    fn pack_bytes_apply_roundtrip() {
        std::env::remove_var("WISP_PACK_INCLUDE_KEYS");
        std::env::remove_var("WISP_MASTER_KEY");
        let dir = tempfile::tempdir().unwrap();
        let a_root = dir.path().join("a");
        let c_root = dir.path().join("c");
        std::fs::create_dir_all(&a_root).unwrap();
        std::fs::write(
            a_root.join("doc.md"),
            "---\ntype: Note\ntitle: P\n---\n\n## One\n\nx\n",
        )
        .unwrap();
        let mut a = Repository::init(&a_root, "0.2", &Unlock::default()).unwrap();
        wisp_engine::record::record(
            &mut a,
            &wisp_engine::record::RecordOptions {
                actor: Some("human:t".into()),
                message: Some("r".into()),
                ..Default::default()
            },
        )
        .unwrap();
        drop(a);
        let mut a = Repository::open(&a_root, &Unlock::default()).unwrap();
        let body = pack_bytes(&mut a).unwrap();
        let mut c = Repository::init(&c_root, "0.2", &Unlock::default()).unwrap();
        apply_pack_bytes(&mut c, &body, &Unlock::default()).unwrap();
    }

    #[test]
    fn cold_pack_omits_keys_by_default() {
        std::env::remove_var("WISP_PACK_INCLUDE_KEYS");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("bundle");
        std::fs::create_dir_all(&root).unwrap();
        let mut repo = Repository::init(&root, "0.2", &Unlock::default()).unwrap();
        std::fs::write(repo.wisp_dir.join("keys/master.key"), b"x").unwrap();
        let pack = dir.path().join("p.zst");
        write_cold_pack(&mut repo, &pack).unwrap();
        let dec = Decoder::new(File::open(&pack).unwrap()).unwrap();
        let mut archive = Archive::new(dec);
        let names: Vec<_> = archive
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.path().ok().map(|p| p.to_string_lossy().to_string()))
            .collect();
        assert!(!names.iter().any(|n| n.contains("keys/")));
    }
}
