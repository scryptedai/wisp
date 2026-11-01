//! Long-lived agent store: `.wisp/runtime.sock`.

#[cfg(test)]
mod acid;
mod apply_writer;
mod cache;
mod client;
mod protocol;
mod serve;

#[cfg(test)]
mod smoke;

pub use cache::SnapshotCache;
pub use client::{
    agents_use_runtime, call, direct_repo_open_allowed, guard_direct_open,
    open_repo_or_misconfigured, require_socket, rpc_apply, rpc_cat, rpc_feed, rpc_impact, rpc_pack,
    rpc_ping, rpc_search, rpc_tokens, rpc_why, set_runtime_serve_thread, test_env_lock,
    RUNTIME_SOCK,
};
pub use protocol::{RuntimeRequest, RuntimeResponse};
pub use serve::{RuntimeHandle, RuntimeServer};
