//! Bind `.wisp/runtime.sock` and dispatch RPCs on a held `Repository`.

use crate::headless::ApplyBatch;
use crate::manifest::RuntimeSection;
use crate::repo::{Repository, Unlock, MAIN_REF};
use crate::runtime::apply_writer::ApplyWriter;
use crate::runtime::cache::SnapshotCache;
use crate::runtime::client::set_runtime_serve_thread;
use crate::runtime::protocol::{RuntimeRequest, RuntimeResponse};
use serde_json::{json, Value};
use std::io::{BufRead, BufWriter, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use wisp_model::{ChangeId, WispError, WriteTimeoutDetails};
use wisp_store::Ledger;

/// Entry point for `wisp runtime --serve` and test harnesses.
pub struct RuntimeServer;

/// Background runtime for tests and embedded use.
pub struct RuntimeHandle {
    join: Mutex<Option<JoinHandle<()>>>,
    sock_path: PathBuf,
}

impl RuntimeHandle {
    /// Wait until `.wisp/runtime.sock` accepts connections or the worker thread exits.
    pub fn wait_ready(&self, timeout: Duration) -> Result<(), WispError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if runtime_sock_ready(&self.sock_path) {
                return Ok(());
            }
            if self
                .join
                .lock()
                .expect("runtime join")
                .as_ref()
                .is_none_or(|j| j.is_finished())
            {
                return Err(WispError::internal(
                    "runtime thread exited before socket ready",
                ));
            }
            if std::time::Instant::now() >= deadline {
                return Err(WispError::internal("runtime.sock not ready"));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for RuntimeHandle {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.sock_path);
        // Drop JoinHandle without joining: accept loop runs until process exit.
        let _ = self.join.lock().expect("runtime join").take();
    }
}

impl RuntimeServer {
    /// Start the accept loop on a worker thread.
    #[cfg(unix)]
    pub fn spawn(
        root: PathBuf,
        unlock: Unlock,
        config: RuntimeSection,
    ) -> Result<Arc<RuntimeHandle>, WispError> {
        let wisp_dir = root.join(crate::repo::WISP_DIR);
        let sock_path = wisp_dir.join(super::client::RUNTIME_SOCK);
        if sock_path.exists() {
            std::fs::remove_file(&sock_path)
                .map_err(|e| WispError::internal(format!("remove old runtime.sock: {e}")))?;
        }
        let sock_wait = sock_path.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(1);
        let join = thread::Builder::new()
            .name("wisp-runtime".into())
            .spawn(move || {
                set_runtime_serve_thread(true);
                if let Err(e) = accept_loop(root, unlock, config, Some(ready_tx)) {
                    eprintln!("wisp runtime: {e}");
                }
            })
            .map_err(|e| WispError::internal(format!("runtime thread: {e}")))?;
        let handle = Arc::new(RuntimeHandle {
            join: Mutex::new(Some(join)),
            sock_path: sock_wait,
        });
        ready_rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| WispError::internal("runtime.sock not ready"))?;
        Ok(handle)
    }

    /// Block in the accept loop on this thread (marks the thread as the runtime store).
    #[cfg(unix)]
    pub fn serve_blocking(
        root: PathBuf,
        unlock: Unlock,
        config: RuntimeSection,
    ) -> Result<(), WispError> {
        crate::runtime::client::set_runtime_serve_thread(true);
        accept_loop(root, unlock, config, None)
    }

    /// Stub on non-Unix hosts (runtime requires domain sockets).
    #[cfg(not(unix))]
    pub fn spawn(
        _root: PathBuf,
        _unlock: Unlock,
        _config: RuntimeSection,
    ) -> Result<Arc<RuntimeHandle>, WispError> {
        Err(WispError::usage(
            "wisp runtime --serve requires Unix domain sockets",
        ))
    }

    /// Stub on non-Unix hosts (runtime requires domain sockets).
    #[cfg(not(unix))]
    pub fn serve_blocking(
        _root: PathBuf,
        _unlock: Unlock,
        _config: RuntimeSection,
    ) -> Result<(), WispError> {
        Err(WispError::usage(
            "wisp runtime --serve requires Unix domain sockets",
        ))
    }
}

#[cfg(unix)]
fn runtime_sock_ready(path: &std::path::Path) -> bool {
    use std::os::unix::net::UnixStream;
    UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn runtime_sock_ready(_path: &std::path::Path) -> bool {
    false
}

#[cfg(unix)]
struct Shared {
    repo: Arc<Mutex<Repository>>,
    cache: Arc<RwLock<SnapshotCache>>,
    config: RuntimeSection,
    apply: ApplyWriter,
}

#[cfg(unix)]
fn accept_loop(
    root: PathBuf,
    unlock: Unlock,
    config: RuntimeSection,
    ready: Option<mpsc::SyncSender<()>>,
) -> Result<(), WispError> {
    let wisp_dir = root.join(crate::repo::WISP_DIR);
    let sock_path = wisp_dir.join(super::client::RUNTIME_SOCK);
    let repo = Arc::new(Mutex::new(Repository::open_worker(&root, &unlock)?));
    let listener = std::os::unix::net::UnixListener::bind(&sock_path)
        .map_err(|e| WispError::internal(format!("bind runtime.sock: {e}")))?;
    if let Some(tx) = ready {
        let _ = tx.send(());
    }
    let cache = Arc::new(RwLock::new(SnapshotCache::default()));
    {
        let mut c = cache.write().expect("cache");
        if let Ok(repo) = repo.lock() {
            if let Ok(h) = repo.head(MAIN_REF) {
                c.seed_head(MAIN_REF, h.map(|x| x.to_string()));
            }
        }
    }
    let apply = ApplyWriter::spawn(repo.clone(), config.clone(), cache.clone());
    let state = Arc::new(Shared {
        repo,
        cache,
        config,
        apply,
    });
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let state = state.clone();
        thread::spawn(move || handle_client(stream, state));
    }
    Ok(())
}

#[cfg(unix)]
fn handle_client(stream: std::os::unix::net::UnixStream, state: Arc<Shared>) {
    let mut reader = std::io::BufReader::new(&stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    drop(reader);
    let resp = match serde_json::from_str::<RuntimeRequest>(&line) {
        Ok(req) => dispatch(req, &state),
        Err(e) => RuntimeResponse::err(
            "runtime",
            json!({ "code": "usage", "message": format!("bad request json: {e}") }),
        ),
    };
    let mut w = BufWriter::new(&stream);
    if let Ok(s) = serde_json::to_string(&resp) {
        let _ = writeln!(w, "{s}");
        let _ = w.flush();
    }
}

#[cfg(unix)]
fn dispatch(req: RuntimeRequest, state: &Arc<Shared>) -> RuntimeResponse {
    match req.cmd.as_str() {
        "ping" => {
            let cache = state.cache.read().expect("cache");
            let seq = cache.seq;
            let head = cache.head(MAIN_REF);
            drop(cache);
            if head.is_some() {
                return RuntimeResponse::ok("ping", json!({ "seq": seq, "head": head }));
            }
            match try_repo_read(state) {
                Ok(repo) => match snapshot_meta(&repo, MAIN_REF) {
                    Ok((_seq, h)) => {
                        if let Some(ref head) = h {
                            state.cache.write().expect("cache").set_head(MAIN_REF, head);
                        }
                        RuntimeResponse::ok("ping", json!({ "seq": seq, "head": h }))
                    }
                    Err(e) => runtime_err("ping", e),
                },
                Err(e) => runtime_err("ping", e),
            }
        }
        "cat" => {
            let ref_name = req
                .args
                .get("ref")
                .and_then(|v| v.as_str())
                .unwrap_or(MAIN_REF)
                .to_string();
            let path = match req.args.get("path").and_then(|v| v.as_str()) {
                Some(p) => p.to_string(),
                None => {
                    return RuntimeResponse::err(
                        "cat",
                        json!({ "code": "usage", "message": "args.path required" }),
                    );
                }
            };
            let version = req
                .args
                .get("version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            handle_cat(state, &ref_name, &path, version.as_deref())
        }
        "apply" => handle_apply(state, req.args, req.timeout_ms),
        "pack" => handle_pack(state, req.args),
        "search" => handle_search(state, req.args),
        "why" => handle_why(state, req.args),
        "impact" => handle_impact(state, req.args),
        "tokens" => handle_tokens(state, req.args),
        "feed" => handle_feed(state, req.args),
        other => RuntimeResponse::err(
            other,
            json!({ "code": "usage", "message": format!("unknown cmd {other}") }),
        ),
    }
}

#[cfg(unix)]
fn handle_cat(
    state: &Arc<Shared>,
    ref_name: &str,
    path: &str,
    version: Option<&str>,
) -> RuntimeResponse {
    if version.is_none() {
        let key = SnapshotCache::cat_key(ref_name, path, None);
        if let Ok(cache) = state.cache.read() {
            if let Some(hit) = cache.get_cat(&key).cloned() {
                let seq = cache.seq;
                let head = cache.head(ref_name);
                drop(cache);
                let mut out = hit;
                stamp_cat_hit(&mut out, seq, head.as_deref());
                return RuntimeResponse::ok("cat", out);
            }
        }
    }
    let version_id = version
        .map(|s| {
            s.parse::<ChangeId>()
                .map_err(|_| WispError::usage("bad version"))
        })
        .transpose();
    let version_id = match version_id {
        Ok(v) => v,
        Err(e) => return runtime_err("cat", e),
    };
    let mut repo = match try_repo_read(state) {
        Ok(g) => g,
        Err(e) => return runtime_err("cat", e),
    };
    match crate::headless::cat(&mut repo, ref_name, path, version_id) {
        Ok(mut v) => {
            if let Some(h) = v.get("head").and_then(|x| x.as_str()) {
                state.cache.write().expect("cache").set_head(ref_name, h);
            }
            let mut cache = state.cache.write().expect("cache");
            let seq = cache.seq;
            if version.is_none() {
                let key = SnapshotCache::cat_key(ref_name, path, None);
                cache.put_cat(key, v.clone());
            }
            if let Some(obj) = v.as_object_mut() {
                obj.insert("seq".into(), json!(seq));
                obj.insert("cache".into(), json!("miss"));
            }
            RuntimeResponse::ok("cat", v)
        }
        Err(e) => runtime_err("cat", e),
    }
}

#[cfg(unix)]
fn stamp_cat_hit(v: &mut Value, seq: u64, head: Option<&str>) {
    if let Some(obj) = v.as_object_mut() {
        obj.insert("seq".into(), json!(seq));
        if let Some(h) = head {
            obj.insert("head".into(), json!(h));
        }
        obj.insert("cache".into(), json!("hit"));
    }
}

#[cfg(unix)]
fn try_repo_read(state: &Arc<Shared>) -> Result<std::sync::MutexGuard<'_, Repository>, WispError> {
    match state.repo.try_lock() {
        Ok(g) => Ok(g),
        Err(std::sync::TryLockError::WouldBlock) => Err(WispError::write_timeout(
            "runtime store busy during read (commit in progress); retry",
            WriteTimeoutDetails {
                lock: "runtime_repo".into(),
                phase: "runtime.read_try_lock".into(),
                hint: Some("Retry shortly; cache-warm reads avoid the store lock entirely.".into()),
                ..Default::default()
            },
        )),
        Err(std::sync::TryLockError::Poisoned(_)) => {
            Err(WispError::internal("runtime repo lock poisoned"))
        }
    }
}

#[cfg(unix)]
fn handle_pack(state: &Arc<Shared>, args: Value) -> RuntimeResponse {
    let ref_name = args
        .get("ref")
        .and_then(|v| v.as_str())
        .unwrap_or(MAIN_REF)
        .to_string();
    let paths: Vec<String> = args
        .get("paths")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let budget = args
        .get("budget")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let since_seq = args.get("since_seq").and_then(|v| v.as_u64());
    let body = args.get("body").and_then(|v| v.as_bool()).unwrap_or(true);
    let key = SnapshotCache::pack_key(&ref_name, &paths, budget);
    let seq = state.cache.read().expect("cache").seq;
    let head_now = head_of(state, &ref_name);
    if since_seq == Some(seq) && !body {
        return RuntimeResponse::ok(
            "pack",
            json!({ "fresh": false, "seq": seq, "head": head_now }),
        );
    }
    if let Some(since) = since_seq {
        if since < seq {
            let delta = state
                .cache
                .read()
                .expect("cache")
                .pack_delta_since(&ref_name, &paths, budget, since, seq);
            if let Some(delta) = delta {
                let head = head_of(state, &ref_name);
                if body {
                    let hit = state
                        .cache
                        .read()
                        .expect("cache")
                        .get_pack(&key)
                        .and_then(|hit| {
                            if hit.seq == seq {
                                Some(hit.value.clone())
                            } else {
                                None
                            }
                        });
                    if let Some(mut out) = hit {
                        stamp_pack(&mut out, seq, head.as_deref(), true, Some(&delta));
                        return RuntimeResponse::ok("pack", out);
                    }
                } else {
                    return RuntimeResponse::ok(
                        "pack",
                        json!({
                            "fresh": true,
                            "seq": seq,
                            "head": head,
                            "delta": delta,
                        }),
                    );
                }
            }
        }
    }
    if let Some(mut out) = state
        .cache
        .read()
        .expect("cache")
        .get_pack(&key)
        .and_then(|hit| {
            if hit.seq == seq {
                Some(hit.value.clone())
            } else {
                None
            }
        })
    {
        stamp_pack(&mut out, seq, head_now.as_deref(), false, None);
        if !body {
            strip_pack_bodies(&mut out);
        }
        return RuntimeResponse::ok("pack", out);
    }
    let seq_before = seq;
    let mut repo = match try_repo_read(state) {
        Ok(g) => g,
        Err(e) => return runtime_err("pack", e),
    };
    match crate::query::pack(&mut repo, &ref_name, &paths, budget) {
        Ok(mut v) => {
            let head = repo.head(&ref_name).ok().flatten().map(|h| h.to_string());
            drop(repo);
            let seq_after = state.cache.read().expect("cache").seq;
            if seq_after == seq_before {
                state.cache.write().expect("cache").put_pack(
                    key,
                    seq_after,
                    paths.clone(),
                    budget,
                    v.clone(),
                );
            }
            stamp_pack(&mut v, seq_after, head.as_deref(), true, None);
            if !body {
                strip_pack_bodies(&mut v);
            }
            RuntimeResponse::ok("pack", v)
        }
        Err(e) => runtime_err("pack", e),
    }
}

#[cfg(unix)]
fn head_of(state: &Arc<Shared>, ref_name: &str) -> Option<String> {
    if let Ok(cache) = state.cache.read() {
        if let Some(h) = cache.head(ref_name) {
            return Some(h);
        }
    }
    state
        .repo
        .lock()
        .ok()
        .and_then(|repo| repo.head(ref_name).ok().flatten())
        .map(|h| h.to_string())
}

#[cfg(unix)]
fn stamp_pack(v: &mut Value, seq: u64, head: Option<&str>, fresh: bool, delta: Option<&Value>) {
    if let Some(obj) = v.as_object_mut() {
        obj.insert("seq".into(), json!(seq));
        obj.insert("fresh".into(), json!(fresh));
        if let Some(h) = head {
            obj.insert("head".into(), json!(h));
        }
        if let Some(d) = delta {
            obj.insert("delta".into(), d.clone());
        }
    }
}

#[cfg(unix)]
fn strip_pack_bodies(v: &mut Value) {
    if let Some(concepts) = v.get_mut("concepts").and_then(|x| x.as_array_mut()) {
        for c in concepts {
            if let Some(sections) = c.get_mut("sections").and_then(|x| x.as_array_mut()) {
                for s in sections {
                    if let Some(obj) = s.as_object_mut() {
                        obj.remove("text");
                    }
                }
            }
        }
    }
}

#[cfg(unix)]
fn handle_apply(state: &Arc<Shared>, args: Value, timeout_ms: Option<u64>) -> RuntimeResponse {
    let batch: ApplyBatch = match serde_json::from_value(args) {
        Ok(b) => b,
        Err(e) => {
            return RuntimeResponse::err(
                "apply",
                json!({ "code": "usage", "message": format!("apply args: {e}") }),
            );
        }
    };
    let default_ref = batch.r#ref.clone();
    let timeout = timeout_ms.unwrap_or(state.config.timeout_ms).max(1);
    match state.apply.submit(batch, default_ref.as_deref(), timeout) {
        Ok(v) => RuntimeResponse::ok("apply", v),
        Err(e) => runtime_err("apply", e),
    }
}

#[cfg(unix)]
fn handle_search(state: &Arc<Shared>, args: Value) -> RuntimeResponse {
    let ref_name = args
        .get("ref")
        .and_then(|v| v.as_str())
        .unwrap_or(MAIN_REF)
        .to_string();
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) => q.to_string(),
        None => {
            return RuntimeResponse::err(
                "search",
                json!({ "code": "usage", "message": "args.query required" }),
            );
        }
    };
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("hybrid")
        .to_string();
    let k = args.get("k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let local_only = args
        .get("local_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let key = SnapshotCache::search_key(&ref_name, &query, &mode, k, local_only);
    let seq = state.cache.read().expect("cache").seq;
    if let Ok(cache) = state.cache.read() {
        if let Some(hit) = cache.get_search(&key).cloned() {
            let mut out = hit;
            if let Some(obj) = out.as_object_mut() {
                obj.insert("seq".into(), json!(seq));
            }
            return RuntimeResponse::ok("search", out);
        }
    }
    let params = crate::search::SearchParams {
        threshold: args
            .get("threshold")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32),
        local_only,
    };
    let mut repo = match try_repo_read(state) {
        Ok(g) => g,
        Err(e) => return runtime_err("search", e),
    };
    match crate::query::search_with_params(&mut repo, &ref_name, &query, &mode, k, &params) {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("seq".into(), json!(seq));
            }
            state
                .cache
                .write()
                .expect("cache")
                .put_search(key, v.clone());
            RuntimeResponse::ok("search", v)
        }
        Err(e) => runtime_err("search", e),
    }
}

#[cfg(unix)]
fn handle_why(state: &Arc<Shared>, args: Value) -> RuntimeResponse {
    let ref_name = args
        .get("ref")
        .and_then(|v| v.as_str())
        .unwrap_or(MAIN_REF)
        .to_string();
    let target = match args.get("target").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => {
            return RuntimeResponse::err(
                "why",
                json!({ "code": "usage", "message": "args.target required" }),
            );
        }
    };
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let mut repo = match try_repo_read(state) {
        Ok(g) => g,
        Err(e) => return runtime_err("why", e),
    };
    match crate::query::why(&mut repo, &ref_name, target, depth) {
        Ok(mut v) => {
            let seq = state.cache.read().expect("cache").seq;
            if let Some(obj) = v.as_object_mut() {
                obj.insert("seq".into(), json!(seq));
            }
            RuntimeResponse::ok("why", v)
        }
        Err(e) => runtime_err("why", e),
    }
}

#[cfg(unix)]
fn handle_impact(state: &Arc<Shared>, args: Value) -> RuntimeResponse {
    let ref_name = args
        .get("ref")
        .and_then(|v| v.as_str())
        .unwrap_or(MAIN_REF)
        .to_string();
    let target = match args.get("target").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => {
            return RuntimeResponse::err(
                "impact",
                json!({ "code": "usage", "message": "args.target required" }),
            );
        }
    };
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let mut repo = match try_repo_read(state) {
        Ok(g) => g,
        Err(e) => return runtime_err("impact", e),
    };
    match crate::impact::impact(&mut repo, &ref_name, target, depth) {
        Ok(mut v) => {
            let seq = state.cache.read().expect("cache").seq;
            if let Some(obj) = v.as_object_mut() {
                obj.insert("seq".into(), json!(seq));
            }
            RuntimeResponse::ok("impact", v)
        }
        Err(e) => runtime_err("impact", e),
    }
}

#[cfg(unix)]
fn handle_tokens(state: &Arc<Shared>, args: Value) -> RuntimeResponse {
    let ref_name = args
        .get("ref")
        .and_then(|v| v.as_str())
        .unwrap_or(MAIN_REF)
        .to_string();
    let target = match args.get("target").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => {
            return RuntimeResponse::err(
                "tokens",
                json!({ "code": "usage", "message": "args.target required" }),
            );
        }
    };
    let mut repo = match try_repo_read(state) {
        Ok(g) => g,
        Err(e) => return runtime_err("tokens", e),
    };
    match crate::query::tokens(&mut repo, &ref_name, target) {
        Ok(mut v) => {
            let seq = state.cache.read().expect("cache").seq;
            if let Some(obj) = v.as_object_mut() {
                obj.insert("seq".into(), json!(seq));
            }
            RuntimeResponse::ok("tokens", v)
        }
        Err(e) => runtime_err("tokens", e),
    }
}

#[cfg(unix)]
fn handle_feed(state: &Arc<Shared>, args: Value) -> RuntimeResponse {
    let since = args.get("since_seq").and_then(|v| v.as_i64()).unwrap_or(0);
    let repo = match try_repo_read(state) {
        Ok(g) => g,
        Err(e) => return runtime_err("feed", e),
    };
    match crate::query::feed(&repo, since) {
        Ok(events) => {
            let head = Ledger::max_seq(repo.ledger.conn()).unwrap_or(0);
            let runtime_seq = state.cache.read().expect("cache").seq;
            RuntimeResponse::ok(
                "feed",
                json!({
                    "since_seq": since,
                    "seq": head,
                    "runtime_seq": runtime_seq,
                    "events": events,
                }),
            )
        }
        Err(e) => runtime_err("feed", e),
    }
}

#[cfg(unix)]
fn snapshot_meta(repo: &Repository, ref_name: &str) -> Result<(u64, Option<String>), WispError> {
    let head = repo.head(ref_name)?.map(|h| h.to_string());
    Ok((0, head))
}

#[cfg(unix)]
fn runtime_err(cmd: &str, e: WispError) -> RuntimeResponse {
    RuntimeResponse::err(
        cmd,
        json!({
            "code": e.code.name(),
            "message": e.message,
            "details": e.details,
        }),
    )
}
