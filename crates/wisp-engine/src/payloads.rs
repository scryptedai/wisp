//! Plaintext encodings of each payload kind (what lives inside the encrypted envelope).

use serde::{Deserialize, Serialize};
use wisp_model::*;

/// Section content plus, for `add_section`/`split`/`move_section`, the position (§8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionBody {
    pub level: u8,
    pub heading: String,
    pub heading_line: String,
    pub body: String,
    /// The section after which this one is inserted. None = first after the preamble.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<SectionId>,
    /// True when `after` means "at the very start" (before any preamble) — unused for now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub at_start: bool,
}

impl SectionBody {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("serializable")
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        serde_json::from_slice(b).map_err(|e| WispError::integrity(format!("section payload: {e}")))
    }
}

/// Delta for `append_section` / `prepend_section` (§8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionFragment {
    pub content: String,
}

impl SectionFragment {
    /// JSON bytes stored as a section-fragment payload.
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("serializable")
    }
    /// Parse a section-fragment payload from cold storage.
    pub fn decode(b: &[u8]) -> Result<Self> {
        serde_json::from_slice(b)
            .map_err(|e| WispError::integrity(format!("section fragment: {e}")))
    }
}

fn ensure_nl(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    if s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

/// Join section body with a fragment (logging-style patches).
pub fn stitch_section_body(existing: &str, fragment: &str, prepend: bool) -> String {
    let frag = ensure_nl(fragment);
    if existing.is_empty() {
        return frag;
    }
    let base = ensure_nl(existing);
    if prepend {
        format!("{frag}{base}")
    } else {
        format!("{base}{frag}")
    }
}

pub fn encode_frontmatter(key: &str, raw: &str) -> Vec<u8> {
    serde_json::to_vec(&FrontmatterPayload {
        key: key.to_string(),
        raw: raw.to_string(),
    })
    .expect("serializable")
}
pub fn decode_frontmatter(b: &[u8]) -> Result<FrontmatterPayload> {
    serde_json::from_slice(b).map_err(|e| WispError::integrity(format!("frontmatter payload: {e}")))
}

pub fn encode_source(id: &str, record: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&SourcePayload {
        id: id.to_string(),
        record: record.clone(),
    })
    .expect("serializable")
}
pub fn decode_source(b: &[u8]) -> Result<SourcePayload> {
    serde_json::from_slice(b).map_err(|e| WispError::integrity(format!("source payload: {e}")))
}

pub fn encode_order(order: &[SectionId]) -> Vec<u8> {
    serde_json::to_vec(order).expect("serializable")
}
pub fn decode_order(b: &[u8]) -> Result<Vec<SectionId>> {
    serde_json::from_slice(b).map_err(|e| WispError::integrity(format!("order payload: {e}")))
}

pub fn encode_group_member(kind: &str, member: &str) -> Vec<u8> {
    serde_json::to_vec(&GroupMemberPayload {
        kind: kind.to_string(),
        member: member.to_string(),
    })
    .expect("serializable")
}
pub fn decode_group_member(b: &[u8]) -> Result<GroupMemberPayload> {
    serde_json::from_slice(b)
        .map_err(|e| WispError::integrity(format!("group member payload: {e}")))
}

/// Asset payload: `u32 be path_len | path | bytes`.
pub fn encode_asset(path: &str, bytes: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + path.len() + bytes.len());
    v.extend_from_slice(&(path.len() as u32).to_be_bytes());
    v.extend_from_slice(path.as_bytes());
    v.extend_from_slice(bytes);
    v
}
pub fn decode_asset(b: &[u8]) -> Result<(String, Vec<u8>)> {
    if b.len() < 4 {
        return Err(WispError::integrity("asset payload too short"));
    }
    let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
    if b.len() < 4 + n {
        return Err(WispError::integrity("asset payload truncated"));
    }
    let path = String::from_utf8(b[4..4 + n].to_vec())
        .map_err(|_| WispError::integrity("asset path not utf-8"))?;
    Ok((path, b[4 + n..].to_vec()))
}
