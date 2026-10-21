//! `wisp-model`: core types shared by every other crate.
//!
//! Metadata records contain only IDs, enums, timestamps, and commitments (§5).
//! All user-authored text lives in encrypted payloads.

pub mod canonical;
pub mod error;
pub mod ids;
pub mod ops;

pub use error::{ExitCode, Result, WispError};
pub use ids::*;
pub use ops::*;

/// RFC 3339 UTC timestamp of now, second precision, `Z` suffix.
pub fn now_rfc3339() -> String {
    let t = time::OffsetDateTime::now_utc();
    let t = t.replace_nanosecond(0).unwrap_or(t);
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Validate an RFC 3339 timestamp (with an offset).
pub fn parse_rfc3339(s: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

/// Normalize a working-tree relative path to the OKF concept path form (forward slashes, no `./`).
pub fn normalize_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    let p = p.strip_prefix("./").unwrap_or(&p);
    p.trim_start_matches('/').to_string()
}

/// OKF concept ID = file path minus `.md` (OKF §2). The mutable alias in wisp.
pub fn concept_path_of(file_path: &str) -> String {
    let n = normalize_path(file_path);
    n.strip_suffix(".md").map(|s| s.to_string()).unwrap_or(n)
}

/// `index.md` and `log.md` are reserved at every level (OKF §3.1, §8).
pub fn is_reserved_file(file_path: &str) -> bool {
    let n = normalize_path(file_path);
    let name = n.rsplit('/').next().unwrap_or(&n);
    name == "index.md" || name == "log.md" || name == ".wisp-checkout.idx"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(concept_path_of("./tables/orders.md"), "tables/orders");
        assert!(is_reserved_file("a/b/index.md"));
        assert!(is_reserved_file("log.md"));
        assert!(!is_reserved_file("a/index2.md"));
        assert!(is_reserved_file(".wisp-checkout.idx"));
        assert!(is_reserved_file("worktrees/x/.wisp-checkout.idx"));
    }

    #[test]
    fn actor_kinds() {
        assert_eq!(ActorKind::classify("human:alex"), Some(ActorKind::Human));
        assert_eq!(
            ActorKind::classify("process:wisp-import"),
            Some(ActorKind::Process)
        );
        assert_eq!(ActorKind::classify("sl-bot/0.3"), Some(ActorKind::Producer));
        assert_eq!(ActorKind::classify("nope"), None);
    }

    #[test]
    fn time_roundtrip() {
        let s = now_rfc3339();
        assert!(parse_rfc3339(&s).is_some());
        assert!(s.ends_with('Z'));
    }
}
