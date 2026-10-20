//! Local Ed25519 signing seeds for changeset signatures (§10.3).

use crate::repo::Reader;
use std::path::{Path, PathBuf};
use wisp_model::{ActorId, Result, WispError};
use wisp_store::Ledger;

/// Optional 32-byte seed (hex) used when no per-actor `.seed` file exists (tests/CI).
pub const ENV_SIGNING_SEED: &str = "WISP_SIGNING_SEED";

pub fn seed_path(wisp_dir: &Path, actor: ActorId) -> PathBuf {
    wisp_dir
        .join("keys")
        .join("signing")
        .join(format!("{actor}.seed"))
}

fn parse_seed_hex(text: &str) -> Result<[u8; 32]> {
    let b = hex::decode(text.trim())
        .map_err(|_| WispError::usage("signing seed must be 64 hex characters (32 bytes)"))?;
    b.try_into()
        .map_err(|_| WispError::usage("signing seed must be 32 bytes"))
}

fn load_seed_bytes(wisp_dir: &Path, actor: ActorId) -> Result<Option<[u8; 32]>> {
    let path = seed_path(wisp_dir, actor);
    if path.is_file() {
        let text = std::fs::read_to_string(&path)?;
        return Ok(Some(parse_seed_hex(&text)?));
    }
    if let Ok(v) = std::env::var(ENV_SIGNING_SEED) {
        return Ok(Some(parse_seed_hex(&v)?));
    }
    Ok(None)
}

/// Persist a signing seed under `.wisp/keys/signing/<actor>.seed` (mode 0600 on Unix).
pub fn save_signing_seed(wisp_dir: &Path, actor: ActorId, seed: &[u8; 32]) -> Result<()> {
    let path = seed_path(wisp_dir, actor);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, hex::encode(seed))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Sign `digest` when a local seed exists and matches the actor's registered public key (if any).
pub fn sign_digest(
    wisp_dir: &Path,
    reader: &Reader<'_>,
    actor: ActorId,
    digest: &[u8; 32],
) -> Result<Option<Vec<u8>>> {
    let Some(seed) = load_seed_bytes(wisp_dir, actor)? else {
        return Ok(None);
    };
    let key = wisp_crypto::sig::from_seed(&seed);
    let pk = wisp_crypto::sig::verify_key_bytes(&key);
    if let Some(row) = Ledger::actor(reader.conn, actor)? {
        if let Some(ref_payload) = row.public_key {
            let stored = reader
                .read_payload(row.scope, &ref_payload)?
                .ok_or_else(|| WispError::integrity("actor public key payload missing"))?;
            if stored.as_slice() != pk.as_slice() {
                return Err(WispError::usage(
                    "local signing seed does not match the actor's registered public key",
                ));
            }
        }
    }
    Ok(Some(wisp_crypto::sig::sign(&key, digest)))
}
