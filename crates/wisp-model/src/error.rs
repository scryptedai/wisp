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
        Self::new(ExitCode::WriteTimeout, msg)
            .with_details(serde_json::json!({ "retryable": true }))
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
