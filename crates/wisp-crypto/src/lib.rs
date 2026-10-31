//! `wisp-crypto`: scope keys, payload encryption, commitments, signatures, keystore (§10).
//!
//! Each scope has a 32-byte random key `K_s`. Scope keys are never derived from one another, so
//! destroying a key cannot be undone with any other key. Working keys are derived with
//! HKDF-SHA256, salt = RepoId.

pub mod keystore;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use wisp_model::{Commit, PayloadId, PayloadKind, RepoId, ScopeId, WispError};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub use keystore::{Keystore, MasterKey};

pub const PAYLOAD_MAGIC: &[u8; 4] = b"WSP1";
pub const PAYLOAD_VERSION: u8 = 1;
/// Plaintext cold payload: `magic | version | kind | bytes…` (no nonce or AEAD).
pub const PAYLOAD_VERSION_PLAIN: u8 = 2;
const INFO_ENC: &[u8] = b"wisp/v1/enc";
const INFO_COMMIT: &[u8] = b"wisp/v1/commit";
const INFO_ACTOR_INDEX: &[u8] = b"wisp/v1/actor-index";

/// A 32-byte random scope key. Zeroized on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ScopeKey(pub [u8; 32]);

impl ScopeKey {
    pub fn generate() -> Self {
        let mut k = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut k);
        ScopeKey(k)
    }
    pub fn from_bytes(b: [u8; 32]) -> Self {
        ScopeKey(b)
    }

    fn derive(&self, repo: RepoId, info: &[u8]) -> [u8; 32] {
        let hk = Hkdf::<Sha256>::new(Some(&repo.bytes()), &self.0);
        let mut out = [0u8; 32];
        hk.expand(info, &mut out)
            .expect("32 bytes is a valid HKDF length");
        out
    }
    /// `K_enc = HKDF(K_s, "wisp/v1/enc")`.
    pub fn enc_key(&self, repo: RepoId) -> [u8; 32] {
        self.derive(repo, INFO_ENC)
    }
    /// `K_com = HKDF(K_s, "wisp/v1/commit")`.
    pub fn commit_key(&self, repo: RepoId) -> [u8; 32] {
        self.derive(repo, INFO_COMMIT)
    }
    /// `K_actor_index = HKDF(K_repo, "wisp/v1/actor-index")` (repository scope only).
    pub fn actor_index_key(&self, repo: RepoId) -> [u8; 32] {
        self.derive(repo, INFO_ACTOR_INDEX)
    }
}

/// `HMAC-SHA256(K_com, kind || plaintext)`.
pub fn commitment(k_com: &[u8; 32], kind: PayloadKind, plaintext: &[u8]) -> Commit {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(k_com).expect("any key length");
    mac.update(&[kind as u8]);
    mac.update(plaintext);
    let out = mac.finalize().into_bytes();
    let mut c = [0u8; 32];
    c.copy_from_slice(&out);
    Commit(c)
}

/// `HMAC(K_actor_index, name)` for looking up an actor by name without storing it in plaintext.
pub fn actor_name_index(k_actor_index: &[u8; 32], name: &str) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(k_actor_index).expect("any key length");
    mac.update(name.as_bytes());
    let out = mac.finalize().into_bytes();
    let mut c = [0u8; 32];
    c.copy_from_slice(&out);
    c
}

fn aad(payload: PayloadId, scope: ScopeId, kind: PayloadKind) -> Vec<u8> {
    let mut a = Vec::with_capacity(33);
    a.extend_from_slice(&payload.bytes());
    a.extend_from_slice(&scope.bytes());
    a.push(kind as u8);
    a
}

/// Encrypt a payload into its file format (§10.2):
/// `magic "WSP1" | version u8 = 1 | kind u8 | nonce [24] | ciphertext || tag [16]`.
pub fn encrypt_payload(
    k_enc: &[u8; 32],
    payload: PayloadId,
    scope: ScopeId,
    kind: PayloadKind,
    plaintext: &[u8],
) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(k_enc.into());
    let mut nonce = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let a = aad(payload, scope, kind);
    let ct = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &a,
            },
        )
        .expect("encryption cannot fail with valid lengths");
    let mut out = Vec::with_capacity(4 + 1 + 1 + 24 + ct.len());
    out.extend_from_slice(PAYLOAD_MAGIC);
    out.push(PAYLOAD_VERSION);
    out.push(kind as u8);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

/// Store a plaintext cold payload (no encryption).
pub fn store_payload_plain(kind: PayloadKind, plaintext: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 1 + 1 + plaintext.len());
    out.extend_from_slice(PAYLOAD_MAGIC);
    out.push(PAYLOAD_VERSION_PLAIN);
    out.push(kind as u8);
    out.extend_from_slice(plaintext);
    out
}

/// Read the kind tag from a payload file without decrypting (encrypted v1 only).
pub fn payload_kind(bytes: &[u8]) -> Result<PayloadKind, WispError> {
    if bytes.len() < 6 || &bytes[..4] != PAYLOAD_MAGIC {
        return Err(WispError::integrity("payload: bad magic"));
    }
    match bytes[4] {
        PAYLOAD_VERSION => {
            if bytes.len() < 30 {
                return Err(WispError::integrity("payload: truncated"));
            }
            PayloadKind::from_u8(bytes[5])
                .ok_or_else(|| WispError::integrity("payload: unknown kind"))
        }
        PAYLOAD_VERSION_PLAIN => PayloadKind::from_u8(bytes[5])
            .ok_or_else(|| WispError::integrity("payload: unknown kind")),
        v => Err(WispError::integrity(format!(
            "payload: unsupported version {v}"
        ))),
    }
}

fn read_payload_plain(bytes: &[u8]) -> Result<(PayloadKind, Vec<u8>), WispError> {
    if bytes.len() < 6 || &bytes[..4] != PAYLOAD_MAGIC {
        return Err(WispError::integrity("payload: bad magic"));
    }
    if bytes[4] != PAYLOAD_VERSION_PLAIN {
        return Err(WispError::integrity(format!(
            "payload: expected plain version {PAYLOAD_VERSION_PLAIN}"
        )));
    }
    let kind = PayloadKind::from_u8(bytes[5])
        .ok_or_else(|| WispError::integrity("payload: unknown kind"))?;
    Ok((kind, bytes[6..].to_vec()))
}

/// Decode a cold payload file for the repository's storage mode.
pub fn read_payload_file(
    encrypted: bool,
    k_enc: Option<&[u8; 32]>,
    payload: PayloadId,
    scope: ScopeId,
    bytes: &[u8],
) -> Result<(PayloadKind, Vec<u8>), WispError> {
    if bytes.len() < 6 || &bytes[..4] != PAYLOAD_MAGIC {
        return Err(WispError::integrity("payload: bad magic"));
    }
    match bytes[4] {
        PAYLOAD_VERSION_PLAIN => read_payload_plain(bytes),
        PAYLOAD_VERSION => {
            if !encrypted {
                return Err(WispError::integrity(
                    "payload: encrypted file in a plain repository",
                ));
            }
            let k = k_enc.ok_or_else(|| WispError::integrity("payload: missing scope key"))?;
            decrypt_payload(k, payload, scope, bytes)
        }
        v => Err(WispError::integrity(format!(
            "payload: unsupported version {v}"
        ))),
    }
}

/// Encode a cold payload for the repository's storage mode.
pub fn store_payload_file(
    encrypted: bool,
    k_enc: &[u8; 32],
    payload: PayloadId,
    scope: ScopeId,
    kind: PayloadKind,
    plaintext: &[u8],
) -> Vec<u8> {
    if encrypted {
        encrypt_payload(k_enc, payload, scope, kind, plaintext)
    } else {
        store_payload_plain(kind, plaintext)
    }
}

/// Decrypt a payload file. Returns (kind, plaintext).
pub fn decrypt_payload(
    k_enc: &[u8; 32],
    payload: PayloadId,
    scope: ScopeId,
    bytes: &[u8],
) -> Result<(PayloadKind, Vec<u8>), WispError> {
    let kind = payload_kind(bytes)?;
    let nonce = &bytes[6..30];
    let ct = &bytes[30..];
    let cipher = XChaCha20Poly1305::new(k_enc.into());
    let a = aad(payload, scope, kind);
    let pt = cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad: &a })
        .map_err(|_| WispError::integrity("payload: authentication failed"))?;
    Ok((kind, pt))
}

/// Ed25519 signing helpers for changeset digests.
pub mod sig {
    use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

    pub fn generate() -> SigningKey {
        SigningKey::generate(&mut rand::rngs::OsRng)
    }
    pub fn from_seed(seed: &[u8; 32]) -> SigningKey {
        SigningKey::from_bytes(seed)
    }
    pub fn seed_bytes(key: &SigningKey) -> [u8; 32] {
        key.to_bytes()
    }
    pub fn verify_key_bytes(key: &SigningKey) -> [u8; 32] {
        key.verifying_key().to_bytes()
    }
    pub fn sign(key: &SigningKey, digest: &[u8; 32]) -> Vec<u8> {
        key.sign(digest).to_bytes().to_vec()
    }
    pub fn verify(public: &[u8], digest: &[u8; 32], sig: &[u8]) -> bool {
        let Ok(pk) = <[u8; 32]>::try_from(public) else {
            return false;
        };
        let Ok(vk) = VerifyingKey::from_bytes(&pk) else {
            return false;
        };
        let Ok(s) = Signature::from_slice(sig) else {
            return false;
        };
        vk.verify(digest, &s).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_payload_roundtrip() {
        let bytes = store_payload_plain(PayloadKind::Section, b"hello");
        assert_eq!(&bytes[..4], b"WSP1");
        assert_eq!(bytes[4], PAYLOAD_VERSION_PLAIN);
        let (kind, pt) =
            read_payload_file(false, None, PayloadId::new(), ScopeId::new(), &bytes).unwrap();
        assert_eq!(kind, PayloadKind::Section);
        assert_eq!(pt, b"hello");
    }

    #[test]
    fn payload_roundtrip_and_aad_binding() {
        let repo = RepoId::new();
        let k = ScopeKey::generate();
        let p = PayloadId::new();
        let s = ScopeId::new();
        let enc = k.enc_key(repo);
        let bytes = encrypt_payload(&enc, p, s, PayloadKind::Section, b"hello");
        assert_eq!(&bytes[..4], b"WSP1");
        let (kind, pt) = decrypt_payload(&enc, p, s, &bytes).unwrap();
        assert_eq!(kind, PayloadKind::Section);
        assert_eq!(pt, b"hello");
        // Wrong scope in AAD fails authentication.
        assert!(decrypt_payload(&enc, p, ScopeId::new(), &bytes).is_err());
        // Different scope key cannot decrypt.
        assert!(decrypt_payload(&ScopeKey::generate().enc_key(repo), p, s, &bytes).is_err());
    }

    #[test]
    fn commitments_are_keyed_and_kind_bound() {
        let repo = RepoId::new();
        let k = ScopeKey::generate();
        let com = k.commit_key(repo);
        let a = commitment(&com, PayloadKind::Section, b"x");
        let b = commitment(&com, PayloadKind::Note, b"x");
        assert_ne!(a, b);
        assert_eq!(a, commitment(&com, PayloadKind::Section, b"x"));
        assert_ne!(
            a,
            commitment(
                &ScopeKey::generate().commit_key(repo),
                PayloadKind::Section,
                b"x"
            )
        );
    }

    #[test]
    fn signatures() {
        let k = sig::generate();
        let d = [7u8; 32];
        let s = sig::sign(&k, &d);
        assert!(sig::verify(k.verifying_key().as_bytes(), &d, &s));
        assert!(!sig::verify(k.verifying_key().as_bytes(), &[8u8; 32], &s));
    }
}
