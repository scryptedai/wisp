//! Dumb HTTP transport for cold-tier packs (`GET`/`PUT` `/wisp/v1/pack.zst`).

use crate::pack::{apply_pack_bytes, pack_bytes};
use std::path::Path;
use std::sync::{Arc, Mutex};
use wisp_engine::apply;
use wisp_engine::repo::{Repository, Unlock};
use wisp_model::*;
use wisp_store::Ledger;

const PACK_PATH: &str = "/wisp/v1/pack.zst";

/// Bearer token from `WISP_HTTP_TOKEN` (shared by server and HTTP sync client).
pub fn http_bearer_token() -> Option<String> {
    std::env::var("WISP_HTTP_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
}

fn bind_looks_loopback(bind: &str) -> bool {
    let host = bind.rsplit_once(':').map(|(h, _)| h).unwrap_or(bind);
    host == "127.0.0.1" || host == "localhost" || host == "[::1]" || host == "::1"
}

/// Block serving `GET`/`PUT` on `http://{bind}{PACK_PATH}` until the process exits.
pub fn serve_http(bind: &str, root: &Path, unlock: &Unlock) -> Result<()> {
    if http_bearer_token().is_some() && !bind_looks_loopback(bind) {
        eprintln!(
            "wisp http: warning: WISP_HTTP_TOKEN is set but bind address {bind} is not loopback; use TLS or terminate TLS in front"
        );
    }
    let state = Arc::new(Mutex::new(HttpState {
        root: root.to_path_buf(),
        unlock: unlock.clone(),
    }));
    let server = tiny_http::Server::http(bind)
        .map_err(|e| WispError::internal(format!("http bind {bind}: {e}")))?;
    eprintln!(
        "wisp http: GET/PUT http://{bind}{PACK_PATH} (set WISP_HTTP_TOKEN for Bearer auth; packs omit keys unless WISP_PACK_INCLUDE_KEYS=1)"
    );
    for request in server.incoming_requests() {
        if let Err(e) = dispatch(&state, request) {
            eprintln!("wisp http: {e}");
        }
    }
    Ok(())
}

struct HttpState {
    root: std::path::PathBuf,
    unlock: Unlock,
}

fn authorize(request: &tiny_http::Request) -> bool {
    match http_bearer_token() {
        None => true,
        Some(expected) => request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .and_then(|h| h.value.as_str().strip_prefix("Bearer "))
            .map(|t| t == expected)
            .unwrap_or(false),
    }
}

fn dispatch(state: &Arc<Mutex<HttpState>>, mut request: tiny_http::Request) -> Result<()> {
    if !authorize(&request) {
        request
            .respond(tiny_http::Response::from_string("unauthorized").with_status_code(401))
            .map_err(|e| WispError::internal(e.to_string()))?;
        return Ok(());
    }
    let url = request.url();
    if url != PACK_PATH {
        request
            .respond(tiny_http::Response::from_string("not found").with_status_code(404))
            .map_err(|e| WispError::internal(e.to_string()))?;
        return Ok(());
    }
    let method = request.method();
    let guard = state.lock().map_err(|_| WispError::internal("http lock"))?;
    let mut repo = Repository::open(&guard.root, &guard.unlock)?;
    if method == &tiny_http::Method::Get {
        let body = pack_bytes(&repo)?;
        request
            .respond(tiny_http::Response::from_data(body).with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], b"application/zstd").unwrap(),
            ))
            .map_err(|e| WispError::internal(e.to_string()))?;
        return Ok(());
    }
    if method == &tiny_http::Method::Put {
        let mut body = Vec::new();
        request
            .as_reader()
            .read_to_end(&mut body)
            .map_err(|e| WispError::internal(e.to_string()))?;
        apply_pack_bytes(&mut repo, &body, &guard.unlock)?;
        if Ledger::head(repo.ledger.conn(), wisp_engine::MAIN_REF)?.is_some() {
            apply::rebuild_hot(&mut repo, wisp_engine::MAIN_REF)?;
        }
        request
            .respond(tiny_http::Response::from_string("ok").with_status_code(200))
            .map_err(|e| WispError::internal(e.to_string()))?;
        return Ok(());
    }
    request
        .respond(tiny_http::Response::from_string("method not allowed").with_status_code(405))
        .map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}
