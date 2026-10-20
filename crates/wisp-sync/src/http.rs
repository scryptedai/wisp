//! Dumb HTTP transport for cold-tier packs (`GET`/`PUT` `/wisp/v1/pack.zst`).

use crate::pack::{apply_pack_bytes, pack_bytes};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use wisp_engine::apply;
use wisp_engine::repo::{Repository, Unlock};
use wisp_model::*;
use wisp_store::Ledger;

const PACK_PATH: &str = "/wisp/v1/pack.zst";

/// Block serving `GET`/`PUT` on `http://{bind}{PACK_PATH}` until the process exits.
pub fn serve_http(bind: &str, root: &Path, unlock: &Unlock) -> Result<()> {
    let state = Arc::new(Mutex::new(HttpState {
        root: root.to_path_buf(),
        unlock: unlock.clone(),
    }));
    let server = tiny_http::Server::http(bind)
        .map_err(|e| WispError::internal(format!("http bind {bind}: {e}")))?;
    eprintln!(
        "wisp http: GET/PUT http://{}{} (trusted transport; add auth in production)",
        bind,
        PACK_PATH
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

fn dispatch(state: &Arc<Mutex<HttpState>>, mut request: tiny_http::Request) -> Result<()> {
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
            .respond(
                tiny_http::Response::from_data(body).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], b"application/zstd")
                        .unwrap(),
                ),
            )
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
