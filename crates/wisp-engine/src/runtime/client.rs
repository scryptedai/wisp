//! Unix socket client for `.wisp/runtime.sock`.

use crate::repo::Unlock;
use crate::runtime::protocol::{RuntimeRequest, RuntimeResponse};
use serde_json::{json, Value};
use std::cell::Cell;
use std::io::{BufRead, BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use wisp_model::WispError;

static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

/// Serialize integration tests that mutate process environment variables.
pub fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_ENV_LOCK.lock().expect("test env lock")
}

thread_local! {
    static RUNTIME_SERVE_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Mark the current thread as the runtime store (may open SQLite).
pub fn set_runtime_serve_thread(active: bool) {
    RUNTIME_SERVE_THREAD.with(|c| c.set(active));
}

fn runtime_serve_thread() -> bool {
    RUNTIME_SERVE_THREAD.with(|c| c.get())
}

/// Socket file name under `.wisp/` (`runtime.sock`).
pub const RUNTIME_SOCK: &str = "runtime.sock";

/// Fail fast when `[runtime] enabled` and the store is not listening.
pub fn require_socket(wisp_dir: &Path) -> Result<(), WispError> {
    let sock = wisp_dir.join(RUNTIME_SOCK);
    if sock.exists() {
        return Ok(());
    }
    Err(WispError::usage(format!(
        "runtime.enabled: start `wisp runtime --serve` (missing {})",
        sock.display()
    ))
    .with_details(json!({ "code": "runtime_unavailable" })))
}

/// Send one JSON-line RPC and return the success payload.
pub fn call(wisp_dir: &Path, req: RuntimeRequest, timeout: Duration) -> Result<Value, WispError> {
    require_socket(wisp_dir)?;
    #[cfg(unix)]
    {
        call_unix(wisp_dir, req, timeout)
    }
    #[cfg(not(unix))]
    {
        let _ = (wisp_dir, req, timeout);
        Err(WispError::usage(
            "runtime socket RPC requires Unix; run the runtime on the same host",
        ))
    }
}

#[cfg(unix)]
fn call_unix(wisp_dir: &Path, req: RuntimeRequest, timeout: Duration) -> Result<Value, WispError> {
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    let sock = wisp_dir.join(RUNTIME_SOCK);
    let stream = UnixStream::connect(&sock)
        .map_err(|e| WispError::usage(format!("runtime.sock connect: {e}")))?;
    stream
        .set_read_timeout(Some(timeout.saturating_add(Duration::from_secs(2))))
        .map_err(|e| WispError::internal(format!("runtime.sock: {e}")))?;
    let mut w = BufWriter::new(&stream);
    writeln!(
        w,
        "{}",
        serde_json::to_string(&req).map_err(|e| WispError::internal(e.to_string()))?
    )
    .map_err(|e| WispError::internal(format!("runtime.sock write: {e}")))?;
    w.flush()
        .map_err(|e| WispError::internal(format!("runtime.sock flush: {e}")))?;
    let mut line = String::new();
    let mut reader = std::io::BufReader::new(&stream);
    reader
        .read_line(&mut line)
        .map_err(|e| WispError::internal(format!("runtime.sock read: {e}")))?;
    let _ = stream.shutdown(Shutdown::Both);
    let resp: RuntimeResponse = serde_json::from_str(&line)
        .map_err(|e| WispError::internal(format!("runtime.sock response: {e}")))?;
    if resp.ok {
        Ok(resp.data.unwrap_or(Value::Null))
    } else if let Some(err) = resp.error {
        Err(WispError::from_json_value(&err)?)
    } else {
        Err(WispError::internal("runtime.sock: malformed error"))
    }
}

/// Build a `ping` RPC.
pub fn rpc_ping(wisp_dir: &Path, timeout_ms: u64) -> Result<Value, WispError> {
    call(
        wisp_dir,
        RuntimeRequest {
            cmd: "ping".into(),
            actor: None,
            timeout_ms: Some(timeout_ms),
            args: json!({}),
        },
        Duration::from_millis(timeout_ms.max(1)),
    )
}

/// Build a `cat` RPC (used by CLI when runtime is enabled).
pub fn rpc_cat(
    wisp_dir: &Path,
    actor: &str,
    ref_name: &str,
    target: &str,
    version: Option<&str>,
    timeout_ms: u64,
) -> Result<Value, WispError> {
    let mut args = json!({ "ref": ref_name, "path": target });
    if let Some(v) = version {
        args["version"] = json!(v);
    }
    call(
        wisp_dir,
        RuntimeRequest {
            cmd: "cat".into(),
            actor: Some(actor.into()),
            timeout_ms: Some(timeout_ms),
            args,
        },
        Duration::from_millis(timeout_ms.max(1)),
    )
}

/// Build an `apply` RPC.
pub fn rpc_apply(
    wisp_dir: &Path,
    actor: &str,
    batch: Value,
    default_ref: Option<&str>,
    timeout_ms: u64,
) -> Result<Value, WispError> {
    let mut args = batch;
    if let Some(r) = default_ref {
        if let Some(obj) = args.as_object_mut() {
            obj.entry("ref".to_string()).or_insert(json!(r));
        }
    }
    call(
        wisp_dir,
        RuntimeRequest {
            cmd: "apply".into(),
            actor: Some(actor.into()),
            timeout_ms: Some(timeout_ms),
            args,
        },
        Duration::from_millis(timeout_ms.max(1)),
    )
}

/// Build a `pack` RPC.
pub fn rpc_pack(
    wisp_dir: &Path,
    actor: &str,
    ref_name: &str,
    paths: &[String],
    budget: Option<usize>,
    since_seq: Option<u64>,
    body: bool,
    timeout_ms: u64,
) -> Result<Value, WispError> {
    call(
        wisp_dir,
        RuntimeRequest {
            cmd: "pack".into(),
            actor: Some(actor.into()),
            timeout_ms: Some(timeout_ms),
            args: json!({
                "ref": ref_name,
                "paths": paths,
                "budget": budget,
                "since_seq": since_seq,
                "body": body,
            }),
        },
        Duration::from_millis(timeout_ms.max(1)),
    )
}

/// Build a `search` RPC.
pub fn rpc_search(
    wisp_dir: &Path,
    actor: &str,
    ref_name: &str,
    query: &str,
    mode: &str,
    k: usize,
    local_only: bool,
    threshold: Option<f64>,
    timeout_ms: u64,
) -> Result<Value, WispError> {
    call(
        wisp_dir,
        RuntimeRequest {
            cmd: "search".into(),
            actor: Some(actor.into()),
            timeout_ms: Some(timeout_ms),
            args: json!({
                "ref": ref_name,
                "query": query,
                "mode": mode,
                "k": k,
                "local_only": local_only,
                "threshold": threshold,
            }),
        },
        Duration::from_millis(timeout_ms.max(1)),
    )
}

/// Build a `why` RPC.
pub fn rpc_why(
    wisp_dir: &Path,
    actor: &str,
    ref_name: &str,
    target: &str,
    depth: usize,
    timeout_ms: u64,
) -> Result<Value, WispError> {
    call(
        wisp_dir,
        RuntimeRequest {
            cmd: "why".into(),
            actor: Some(actor.into()),
            timeout_ms: Some(timeout_ms),
            args: json!({ "ref": ref_name, "target": target, "depth": depth }),
        },
        Duration::from_millis(timeout_ms.max(1)),
    )
}

/// Build an `impact` RPC.
pub fn rpc_impact(
    wisp_dir: &Path,
    actor: &str,
    ref_name: &str,
    target: &str,
    depth: usize,
    timeout_ms: u64,
) -> Result<Value, WispError> {
    call(
        wisp_dir,
        RuntimeRequest {
            cmd: "impact".into(),
            actor: Some(actor.into()),
            timeout_ms: Some(timeout_ms),
            args: json!({ "ref": ref_name, "target": target, "depth": depth }),
        },
        Duration::from_millis(timeout_ms.max(1)),
    )
}

/// Build a `tokens` RPC.
pub fn rpc_tokens(
    wisp_dir: &Path,
    actor: &str,
    ref_name: &str,
    target: &str,
    timeout_ms: u64,
) -> Result<Value, WispError> {
    call(
        wisp_dir,
        RuntimeRequest {
            cmd: "tokens".into(),
            actor: Some(actor.into()),
            timeout_ms: Some(timeout_ms),
            args: json!({ "ref": ref_name, "target": target }),
        },
        Duration::from_millis(timeout_ms.max(1)),
    )
}

/// Build a `feed` RPC (ledger events since seq).
pub fn rpc_feed(
    wisp_dir: &Path,
    actor: &str,
    since_seq: i64,
    timeout_ms: u64,
) -> Result<Value, WispError> {
    call(
        wisp_dir,
        RuntimeRequest {
            cmd: "feed".into(),
            actor: Some(actor.into()),
            timeout_ms: Some(timeout_ms),
            args: json!({ "since_seq": since_seq }),
        },
        Duration::from_millis(timeout_ms.max(1)),
    )
}

/// True when agents must not open the ledger directly.
pub fn agents_use_runtime(manifest: &crate::manifest::Manifest) -> bool {
    manifest.runtime.enabled
}

/// True when this process may open SQLite (runtime off, serve thread, or override).
pub fn direct_repo_open_allowed(manifest: &crate::manifest::Manifest) -> bool {
    !manifest.runtime.enabled
        || runtime_serve_thread()
        || std::env::var("WISP_RUNTIME_DIRECT").ok().as_deref() == Some("1")
}

/// Reject agent direct opens when runtime is enabled (except allowed humans).
pub fn guard_direct_open(
    manifest: &crate::manifest::Manifest,
    actor: Option<&str>,
) -> Result<(), WispError> {
    if direct_repo_open_allowed(manifest) {
        return Ok(());
    }
    if manifest.runtime.allow_human_direct && actor.is_some_and(|a| a.starts_with("human:")) {
        return Ok(());
    }
    Err(WispError::usage(
        "runtime.enabled: agents must use the runtime socket; start `wisp runtime --serve`",
    )
    .with_details(json!({ "code": "runtime_direct_open_forbidden" })))
}

/// Open the repo or fail when runtime forbids direct SQLite access.
pub fn open_repo_or_misconfigured(
    root: &Path,
    unlock: &Unlock,
) -> Result<crate::repo::Repository, WispError> {
    let wisp_dir = root.join(crate::repo::WISP_DIR);
    if !wisp_dir.is_dir() {
        return crate::repo::Repository::open(root, unlock);
    }
    let manifest = crate::manifest::Manifest::load(&wisp_dir.join("manifest.toml"))?;
    guard_direct_open(&manifest, None)?;
    crate::repo::Repository::open(root, unlock)
}
