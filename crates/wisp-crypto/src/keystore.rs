//! The keystore (§10.1): each `K_s` wrapped with XChaCha20-Poly1305 under a master key.
//!
//! Master key sources, in order: `WISP_MASTER_KEY` (64 hex chars; CI), a passphrase via
//! Argon2id (m = 64 MiB, t = 3, p = 1), or a key file at `.wisp/keys/master.key` with 0600
//! permissions. The OS keychain default from §10.1 is replaced by the key file in this build
//! (DECISIONS.md §10.1).

use crate::ScopeKey;
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use wisp_model::{ScopeId, WispError};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey(pub [u8; 32]);

pub const ENV_MASTER_KEY: &str = "WISP_MASTER_KEY";

impl MasterKey {
    pub fn from_env() -> Result<Option<Self>, WispError> {
        match std::env::var(ENV_MASTER_KEY) {
            Ok(v) => {
                let b = hex::decode(v.trim()).map_err(|_| {
                    WispError::key_unavailable(format!(
                        "{ENV_MASTER_KEY} must be 64 hex characters"
                    ))
                })?;
                let k: [u8; 32] = b.try_into().map_err(|_| {
                    WispError::key_unavailable(format!("{ENV_MASTER_KEY} must be 32 bytes"))
                })?;
                Ok(Some(MasterKey(k)))
            }
            Err(_) => Ok(None),
        }
    }

    pub fn from_passphrase(passphrase: &str, salt: &[u8; 16]) -> Result<Self, WispError> {
        let params = Params::new(64 * 1024, 3, 1, Some(32))
            .map_err(|e| WispError::internal(format!("argon2 params: {e}")))?;
        let a = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut out = [0u8; 32];
        a.hash_password_into(passphrase.as_bytes(), salt, &mut out)
            .map_err(|e| WispError::internal(format!("argon2: {e}")))?;
        Ok(MasterKey(out))
    }

    /// Read or create the key file.
    pub fn from_file(path: &Path, create: bool) -> Result<Self, WispError> {
        if path.exists() {
            let s = std::fs::read_to_string(path)?;
            let b = hex::decode(s.trim())
                .map_err(|_| WispError::key_unavailable("master.key is not hex"))?;
            let k: [u8; 32] = b
                .try_into()
                .map_err(|_| WispError::key_unavailable("master.key must be 32 bytes"))?;
            return Ok(MasterKey(k));
        }
        if !create {
            return Err(WispError::key_unavailable(format!(
                "no master key: {} missing and {} unset",
                path.display(),
                ENV_MASTER_KEY
            )));
        }
        let mut k = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut k);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, hex::encode(k))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(MasterKey(k))
    }
}

/// Wrapped scope keys in SQLite.
pub struct Keystore {
    conn: Connection,
    master: MasterKey,
    path: PathBuf,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS scope_keys (
  scope   BLOB PRIMARY KEY,
  wrapped BLOB NOT NULL
);
";

impl Keystore {
    /// Open (or create) the keystore at `path` with the given master key.
    pub fn open(path: &Path, master: MasterKey) -> Result<Self, WispError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).map_err(sql_err)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA secure_delete = ON; PRAGMA auto_vacuum = INCREMENTAL;",
        )
        .map_err(sql_err)?;
        conn.execute_batch(SCHEMA).map_err(sql_err)?;
        let ks = Keystore {
            conn,
            master,
            path: path.to_path_buf(),
        };
        ks.verify_master()?;
        Ok(ks)
    }

    /// Store a check value so a wrong passphrase is detected early (exit 9).
    fn verify_master(&self) -> Result<(), WispError> {
        let existing: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'check'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(sql_err)?;
        match existing {
            Some(blob) => {
                let pt = self
                    .unwrap_raw(b"check", &blob)
                    .map_err(|_| WispError::key_unavailable("keystore locked: wrong master key"))?;
                if pt != b"wisp-keystore-v1" {
                    return Err(WispError::key_unavailable(
                        "keystore locked: wrong master key",
                    ));
                }
                Ok(())
            }
            None => {
                let blob = self.wrap_raw(b"check", b"wisp-keystore-v1");
                self.conn
                    .execute(
                        "INSERT INTO meta (key, value) VALUES ('check', ?1)",
                        params![blob],
                    )
                    .map_err(sql_err)?;
                Ok(())
            }
        }
    }

    /// The Argon2 salt for passphrase-derived master keys, created on first use.
    pub fn passphrase_salt(path: &Path) -> Result<[u8; 16], WispError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).map_err(sql_err)?;
        conn.execute_batch(SCHEMA).map_err(sql_err)?;
        let existing: Option<Vec<u8>> = conn
            .query_row("SELECT value FROM meta WHERE key = 'salt'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(sql_err)?;
        if let Some(b) = existing {
            let s: [u8; 16] = b
                .try_into()
                .map_err(|_| WispError::integrity("keystore salt corrupt"))?;
            return Ok(s);
        }
        let mut s = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut s);
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('salt', ?1)",
            params![s.to_vec()],
        )
        .map_err(sql_err)?;
        Ok(s)
    }

    fn wrap_raw(&self, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let cipher = XChaCha20Poly1305::new((&self.master.0).into());
        let mut nonce = [0u8; 24];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ct = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("wrap");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        out
    }

    fn unwrap_raw(&self, aad: &[u8], blob: &[u8]) -> Result<Vec<u8>, WispError> {
        if blob.len() < 24 + 16 {
            return Err(WispError::integrity("wrapped key too short"));
        }
        let cipher = XChaCha20Poly1305::new((&self.master.0).into());
        cipher
            .decrypt(
                XNonce::from_slice(&blob[..24]),
                Payload {
                    msg: &blob[24..],
                    aad,
                },
            )
            .map_err(|_| WispError::key_unavailable("cannot unwrap scope key"))
    }

    /// Write a scope key in its own transaction (§10.2 step 1). Idempotent for the same key.
    pub fn put(&self, scope: ScopeId, key: &ScopeKey) -> Result<(), WispError> {
        let wrapped = self.wrap_raw(&scope.bytes(), &key.0);
        self.put_wrapped(scope, &wrapped)
    }

    /// Copy a wrapped scope key from a trusted peer (same master key).
    pub fn put_wrapped(&self, scope: ScopeId, wrapped: &[u8]) -> Result<(), WispError> {
        let pt = self.unwrap_raw(&scope.bytes(), wrapped)?;
        if pt.len() != 32 {
            return Err(WispError::integrity("scope key wrong length"));
        }
        self.conn
            .execute(
                "INSERT OR IGNORE INTO scope_keys (scope, wrapped) VALUES (?1, ?2)",
                params![scope.bytes().to_vec(), wrapped],
            )
            .map_err(sql_err)?;
        Ok(())
    }

    /// Wrapped ciphertext for replication between trusted replicas.
    pub fn wrapped_blob(&self, scope: ScopeId) -> Result<Option<Vec<u8>>, WispError> {
        self.conn
            .query_row(
                "SELECT wrapped FROM scope_keys WHERE scope = ?1",
                params![scope.bytes().to_vec()],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)
    }

    /// Fetch a scope key. `None` when the scope has no key (erased or never existed).
    pub fn get(&self, scope: ScopeId) -> Result<Option<ScopeKey>, WispError> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT wrapped FROM scope_keys WHERE scope = ?1",
                params![scope.bytes().to_vec()],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)?;
        match blob {
            None => Ok(None),
            Some(b) => {
                let pt = self.unwrap_raw(&scope.bytes(), &b)?;
                let k: [u8; 32] = pt
                    .try_into()
                    .map_err(|_| WispError::integrity("scope key wrong length"))?;
                Ok(Some(ScopeKey(k)))
            }
        }
    }

    pub fn has(&self, scope: ScopeId) -> Result<bool, WispError> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM scope_keys WHERE scope = ?1",
                params![scope.bytes().to_vec()],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        Ok(n > 0)
    }

    /// Destroy a scope key (§10.5 Phase 2). `secure_delete` zeroes the freed pages.
    pub fn delete(&self, scope: ScopeId) -> Result<(), WispError> {
        self.conn
            .execute(
                "DELETE FROM scope_keys WHERE scope = ?1",
                params![scope.bytes().to_vec()],
            )
            .map_err(sql_err)?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<ScopeId>, WispError> {
        let mut st = self
            .conn
            .prepare("SELECT scope FROM scope_keys")
            .map_err(sql_err)?;
        let rows = st
            .query_map([], |r| r.get::<_, Vec<u8>>(0))
            .map_err(sql_err)?
            .filter_map(|r| r.ok())
            .filter_map(|b| ScopeId::from_slice(&b))
            .collect();
        Ok(rows)
    }

    /// `PRAGMA wal_checkpoint(TRUNCATE)` so deleted keys do not linger in the WAL.
    pub fn checkpoint(&self) -> Result<(), WispError> {
        self.conn
            .execute_batch("PRAGMA incremental_vacuum; PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(sql_err)?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn sql_err(e: rusqlite::Error) -> WispError {
    WispError::internal(format!("keystore: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_wrapped_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let master = MasterKey::from_file(&dir.path().join("master.key"), true).unwrap();
        let a = Keystore::open(&dir.path().join("a.db"), master.clone()).unwrap();
        let b = Keystore::open(&dir.path().join("b.db"), master).unwrap();
        let s = ScopeId::new();
        let k = ScopeKey::generate();
        a.put(s, &k).unwrap();
        let wrapped = a.wrapped_blob(s).unwrap().unwrap();
        b.put_wrapped(s, &wrapped).unwrap();
        assert_eq!(b.get(s).unwrap().unwrap().0, k.0);
    }

    #[test]
    fn put_wrapped_rejects_bad_ciphertext() {
        let dir = tempfile::tempdir().unwrap();
        let master = MasterKey::from_file(&dir.path().join("master.key"), true).unwrap();
        let ks = Keystore::open(&dir.path().join("keystore.db"), master).unwrap();
        let s = ScopeId::new();
        assert!(ks.put_wrapped(s, b"too-short").is_err());
    }

    #[test]
    fn put_get_delete() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("keystore.db");
        let master = MasterKey::from_file(&dir.path().join("master.key"), true).unwrap();
        let ks = Keystore::open(&p, master.clone()).unwrap();
        let s = ScopeId::new();
        let k = ScopeKey::generate();
        ks.put(s, &k).unwrap();
        assert_eq!(ks.get(s).unwrap().unwrap().0, k.0);
        ks.delete(s).unwrap();
        assert!(ks.get(s).unwrap().is_none());
        drop(ks);
        // Wrong master key is detected.
        let wrong = MasterKey([1u8; 32]);
        assert!(Keystore::open(&p, wrong).is_err());
        // Same master reopens.
        assert!(Keystore::open(&p, master).is_ok());
    }

    #[test]
    fn passphrase_derivation_is_stable() {
        let salt = [3u8; 16];
        let a = MasterKey::from_passphrase("hunter2", &salt).unwrap();
        let b = MasterKey::from_passphrase("hunter2", &salt).unwrap();
        assert_eq!(a.0, b.0);
    }
}
