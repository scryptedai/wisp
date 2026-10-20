//! `wisp-sub-tokens`: token counting (§14.4).
//!
//! This build ships one approximate codec, `tok:approx:chars4` (one token per four characters),
//! implemented inline in the engine so `pack --budget` works without a tokenizer download.
//! `tok:tiktoken:*` and `tok:hf:*` codecs are milestone M5.

pub const APPROX_CODEC: &str = "tok:approx:chars4";

pub fn count(text: &str) -> usize {
    (text.chars().count() + 3) / 4
}
