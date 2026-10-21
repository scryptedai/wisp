//! `wisp-sub-tokens`: token counting (§14.4).
//!
//! Community builds ship built-in **approx** codecs for `wisp pack --budget` without downloads.
//! Products can add `tok:tiktoken:*` / `tok:hf:*` via subscribers (see `docs/EXTENSIONS.md`).

use wisp_model::{Result, WispError};

pub const APPROX_CODEC: &str = "tok:approx:chars4";
pub const APPROX_BYTES_CODEC: &str = "tok:approx:bytes4";

/// Count tokens for packing and CLI (`wisp tokens`).
pub trait TokenCodec: Send + Sync {
    fn id(&self) -> &str;
    fn count(&self, text: &str) -> usize;
}

struct ApproxChars4;
struct ApproxBytes4;

impl TokenCodec for ApproxChars4 {
    fn id(&self) -> &str {
        APPROX_CODEC
    }
    fn count(&self, text: &str) -> usize {
        text.chars().count().div_ceil(4)
    }
}

impl TokenCodec for ApproxBytes4 {
    fn id(&self) -> &str {
        APPROX_BYTES_CODEC
    }
    fn count(&self, text: &str) -> usize {
        text.len().div_ceil(4)
    }
}

static APPROX_CHARS: ApproxChars4 = ApproxChars4;
static APPROX_BYTES: ApproxBytes4 = ApproxBytes4;

/// Resolve a built-in tokenizer id (community release).
pub fn codec_for(id: &str) -> Result<&'static dyn TokenCodec> {
    match id {
        APPROX_CODEC => Ok(&APPROX_CHARS),
        APPROX_BYTES_CODEC => Ok(&APPROX_BYTES),
        other if other.starts_with("tok:") => Err(WispError::usage(format!(
            "tokenizer {other} is not built in; register a subscriber or use tok:approx:chars4"
        ))),
        _ => Err(WispError::usage("tokenizer IDs start with tok:")),
    }
}

/// Count with a named codec (defaults to [`APPROX_CODEC`] when `id` is empty).
pub fn count(id: &str, text: &str) -> Result<usize> {
    let id = if id.is_empty() { APPROX_CODEC } else { id };
    Ok(codec_for(id)?.count(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx_chars_and_bytes_differ_for_unicode() {
        let s = "é"; // 1 char, 2 utf8 bytes
        assert_eq!(count(APPROX_CODEC, s).unwrap(), 1);
        assert_eq!(count(APPROX_BYTES_CODEC, s).unwrap(), 1);
        let s2 = "abcd";
        assert_eq!(count(APPROX_CODEC, s2).unwrap(), 1);
        assert_eq!(count(APPROX_BYTES_CODEC, s2).unwrap(), 1);
    }
}
