//! Typed errors with stable exit codes (§16).

use serde::Serialize;
use serde_json::Value;
use std::fmt;

/// Exit codes (§16).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum ExitCode {
    Success = 0,
    Internal = 1,
    Usage = 2,
    Conflict = 3,
    Refused = 4,
    Validation = 5,
    NotFound = 6,
    WriteTimeout = 7,
    Integrity = 8,
    KeyUnavailable = 9,
    ErasePending = 10,
}

impl ExitCode {
    pub fn code(&self) -> i32 {
        *self as i32
    }
    pub fn name(&self) -> &'static str {
        match self {
            ExitCode::Success => "ok",
            ExitCode::Internal => "internal",
            ExitCode::Usage => "usage",
            ExitCode::Conflict => "conflict",
            ExitCode::Refused => "refused",
            ExitCode::Validation => "validation",
            ExitCode::NotFound => "not_found",
            ExitCode::WriteTimeout => "write_timeout",
            ExitCode::Integrity => "integrity",
            ExitCode::KeyUnavailable => "key_unavailable",
            ExitCode::ErasePending => "erase_pending",
        }
    }
}

/// The one error type every library crate returns. The CLI maps it to `{code, message, details}`.
#[derive(Debug, Clone, Serialize)]
pub struct WispError {
    #[serde(serialize_with = "ser_code")]
    pub code: ExitCode,
    pub message: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

fn ser_code<S: serde::Serializer>(c: &ExitCode, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(c.name())
}

impl WispError {
    pub fn new(code: ExitCode, message: impl Into<String>) -> Self {
        WispError {
            code,
            message: message.into(),
            details: Value::Null,
        }
    }
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(ExitCode::Internal, msg)
    }
    pub fn usage(msg: impl Into<String>) -> Self {
        Self::new(ExitCode::Usage, msg)
    }
    pub fn conflict(msg: impl Into<String>, conflicts: Value) -> Self {
        Self::new(ExitCode::Conflict, msg)
            .with_details(serde_json::json!({ "conflicts": conflicts }))
    }
    pub fn refused(msg: impl Into<String>) -> Self {
        Self::new(ExitCode::Refused, msg)
    }
    pub fn validation(msg: impl Into<String>, findings: Value) -> Self {
        Self::new(ExitCode::Validation, msg)
            .with_details(serde_json::json!({ "findings": findings }))
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(ExitCode::NotFound, msg)
    }
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::write_timeout(msg, WriteTimeoutDetails::default())
    }

    /// Exit 7 with structured fields for agents (`error.details` in `--json`).
    pub fn write_timeout(msg: impl Into<String>, detail: WriteTimeoutDetails) -> Self {
        let details = serde_json::to_value(detail).unwrap_or(Value::Null);
        Self::new(ExitCode::WriteTimeout, msg).with_details(details)
    }

    /// Merge keys into `details` (exit 7 only); no-op for other codes.
    pub fn merge_write_timeout_details(mut self, extra: Value) -> Self {
        if self.code != ExitCode::WriteTimeout {
            return self;
        }
        self.details = merge_json_objects(self.details, extra);
        self
    }
    pub fn integrity(msg: impl Into<String>) -> Self {
        Self::new(ExitCode::Integrity, msg)
    }
    pub fn key_unavailable(msg: impl Into<String>) -> Self {
        Self::new(ExitCode::KeyUnavailable, msg)
    }
    pub fn not_implemented(what: &str) -> Self {
        Self::new(
            ExitCode::Usage,
            format!("{what} is not implemented in this build"),
        )
        .with_details(serde_json::json!({ "not_implemented": what }))
    }
    pub fn is_retryable(&self) -> bool {
        self.code == ExitCode::WriteTimeout
    }
}

impl fmt::Display for WispError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.name(), self.message)
    }
}

impl std::error::Error for WispError {}

impl From<std::io::Error> for WispError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => WispError::not_found(e.to_string()),
            _ => WispError::internal(format!("io: {e}")),
        }
    }
}

impl From<serde_json::Error> for WispError {
    fn from(e: serde_json::Error) -> Self {
        WispError::internal(format!("json: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, WispError>;

/// Agent-facing exit-7 payload (`error.details` when `code` is `write_timeout`).
#[derive(Debug, Clone, Serialize)]
pub struct WriteTimeoutDetails {
    pub retryable: bool,
    /// `ledger`, `hot`, `worktree_flock`, or `unknown`.
    #[serde(default)]
    pub lock: String,
    /// Where the wait failed, e.g. `ledger.begin_immediate`, `commit.try_commit`.
    #[serde(default)]
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_retries_max: Option<u32>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub ref_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sqlite: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Default for WriteTimeoutDetails {
    fn default() -> Self {
        Self {
            retryable: true,
            lock: String::new(),
            phase: String::new(),
            busy_timeout_ms: None,
            commit_attempt: None,
            commit_retries_max: None,
            ref_name: None,
            sqlite: None,
            hint: None,
        }
    }
}

fn merge_json_objects(base: Value, extra: Value) -> Value {
    match (base, extra) {
        (Value::Object(mut a), Value::Object(b)) => {
            for (k, v) in b {
                a.insert(k, v);
            }
            Value::Object(a)
        }
        (_, extra) => extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_timeout_details_serialize() {
        let e = WispError::write_timeout(
            "write queue timeout",
            WriteTimeoutDetails {
                lock: "ledger".into(),
                phase: "ledger.begin_immediate".into(),
                busy_timeout_ms: Some(200),
                hint: Some("retry".into()),
                ..Default::default()
            },
        );
        let v = serde_json::to_value(&e).unwrap();
        let details = v.get("details").unwrap();
        assert_eq!(
            details.get("retryable").and_then(|x| x.as_bool()),
            Some(true)
        );
        assert_eq!(details.get("lock").and_then(|x| x.as_str()), Some("ledger"));
        assert_eq!(
            details.get("busy_timeout_ms").and_then(|x| x.as_u64()),
            Some(200)
        );
    }

    #[test]
    fn merge_write_timeout_details_preserves_lock() {
        let base = WispError::write_timeout(
            "write queue timeout",
            WriteTimeoutDetails {
                lock: "ledger".into(),
                phase: "ledger.begin_immediate".into(),
                busy_timeout_ms: Some(200),
                ..Default::default()
            },
        );
        let merged = base.merge_write_timeout_details(serde_json::json!({
            "commit_attempt": 3,
            "commit_retries_max": 3,
            "ref": "main",
            "phase": "commit.try_commit",
        }));
        let details = merged.details;
        assert_eq!(details.get("lock").and_then(|x| x.as_str()), Some("ledger"));
        assert_eq!(
            details.get("phase").and_then(|x| x.as_str()),
            Some("commit.try_commit")
        );
        assert_eq!(
            details.get("commit_attempt").and_then(|x| x.as_u64()),
            Some(3)
        );
    }
}
