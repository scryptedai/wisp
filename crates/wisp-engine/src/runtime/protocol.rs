//! JSON line protocol for `.wisp/runtime.sock`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One inbound request line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeRequest {
    pub cmd: String,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub args: Value,
}

/// Outbound envelope (matches CLI `--json` success/failure shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeResponse {
    pub ok: bool,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

impl RuntimeResponse {
    /// Success envelope for one RPC.
    pub fn ok(command: &str, data: Value) -> Self {
        RuntimeResponse {
            ok: true,
            command: command.to_string(),
            data: Some(data),
            error: None,
        }
    }

    /// Failure envelope carrying a CLI-shaped error object.
    pub fn err(command: &str, error: Value) -> Self {
        RuntimeResponse {
            ok: false,
            command: command.to_string(),
            data: None,
            error: Some(error),
        }
    }
}
