//! Minted identity. Every ID is a ULID; content hashes are never identity (§3.4).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

macro_rules! ulid_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub ulid::Ulid);

        impl $name {
            /// Mint a fresh ID.
            pub fn new() -> Self {
                Self(ulid::Ulid::new())
            }
            /// The 16 raw bytes.
            pub fn bytes(&self) -> [u8; 16] {
                self.0.to_bytes()
            }
            /// From 16 raw bytes.
            pub fn from_bytes(b: [u8; 16]) -> Self {
                Self(ulid::Ulid::from_bytes(b))
            }
            /// From a byte slice (16 bytes), as stored in SQLite BLOB columns.
            pub fn from_slice(b: &[u8]) -> Option<Self> {
                if b.len() != 16 {
                    return None;
                }
                let mut a = [0u8; 16];
                a.copy_from_slice(b);
                Some(Self::from_bytes(a))
            }
            /// Crockford base32, 26 characters.
            pub fn to_base32(&self) -> String {
                self.0.to_string()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl FromStr for $name {
            type Err = ulid::DecodeError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(ulid::Ulid::from_str(s)?))
            }
        }
    };
}

ulid_id!(RepoId, "One per bundle; owns the manifest.");
ulid_id!(
    ConceptId,
    "One markdown knowledge unit (written as `wisp_id`)."
);
ulid_id!(
    SectionId,
    "Unit of content and change, anchored in the heading line."
);
ulid_id!(AssetId, "Non-markdown file, versioned whole.");
ulid_id!(ScopeId, "Encryption and erasure boundary.");
ulid_id!(PayloadId, "Encrypted content blob.");
ulid_id!(ChangeId, "One operation on one target.");
ulid_id!(ChangesetId, "Atomic group of changes; the DAG node.");
ulid_id!(ActorId, "Who performed a change (opaque in metadata).");
ulid_id!(GroupId, "A named erasure group.");

/// A 32-byte keyed commitment: HMAC-SHA256(K_com, kind || plaintext).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Commit(pub [u8; 32]);

impl Commit {
    pub fn from_slice(b: &[u8]) -> Option<Self> {
        if b.len() != 32 {
            return None;
        }
        let mut a = [0u8; 32];
        a.copy_from_slice(b);
        Some(Commit(a))
    }
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Debug for Commit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Commit({})", &self.to_hex()[..12])
    }
}

impl Serialize for Commit {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Commit {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let b = hex::decode(&s).map_err(serde::de::Error::custom)?;
        Commit::from_slice(&b)
            .ok_or_else(|| serde::de::Error::custom("commitment must be 32 bytes"))
    }
}

/// A reference to an encrypted payload: its ID plus the commitment to its plaintext.
/// Serialized in SQLite as `id (16) || commit (32)`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct PayloadRef {
    pub id: PayloadId,
    pub commit: Commit,
}

impl PayloadRef {
    pub fn to_blob(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(48);
        v.extend_from_slice(&self.id.bytes());
        v.extend_from_slice(&self.commit.0);
        v
    }
    pub fn from_blob(b: &[u8]) -> Option<Self> {
        if b.len() != 48 {
            return None;
        }
        Some(PayloadRef {
            id: PayloadId::from_slice(&b[..16])?,
            commit: Commit::from_slice(&b[16..])?,
        })
    }
}
