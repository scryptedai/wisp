//! Remote URL parsing and transports (git-remote-shaped).

use crate::http::http_bearer_token;
use crate::local::{init_trusted_replica, pull, push};
use crate::pack::{apply_pack_bytes, pack_bytes};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use wisp_engine::apply;
use wisp_engine::repo::{Repository, Unlock};
use wisp_model::*;
use wisp_store::Ledger;

#[derive(Clone, Debug)]
pub enum RemoteTarget {
    Local(PathBuf),
    Http(HttpRemote),
    Ssh(SshRemote),
}

#[derive(Clone, Debug)]
pub struct HttpRemote {
    pub base: String,
}

#[derive(Clone, Debug)]
pub struct SshRemote {
    pub user: Option<String>,
    pub host: String,
    pub path: PathBuf,
}

/// Parse `file://`, plain paths, `https://`, `http://`, and `ssh://user@host/path`.
pub fn parse_remote_url(url: &str) -> Result<RemoteTarget> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("https://") {
        return Ok(RemoteTarget::Http(HttpRemote {
            base: format!("https://{}", rest.trim_end_matches('/')),
        }));
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return Ok(RemoteTarget::Http(HttpRemote {
            base: format!("http://{}", rest.trim_end_matches('/')),
        }));
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        return parse_ssh(rest);
    }
    Ok(RemoteTarget::Local(crate::local::parse_local_url(url)?))
}

fn parse_ssh(rest: &str) -> Result<RemoteTarget> {
    let (auth, path) = rest
        .split_once('/')
        .ok_or_else(|| WispError::usage("ssh URL must be ssh://[user@]host/path"))?;
    let (user, host) = if let Some((u, h)) = auth.split_once('@') {
        (Some(u.to_string()), h)
    } else {
        (None, auth)
    };
    Ok(RemoteTarget::Ssh(SshRemote {
        user,
        host: host.to_string(),
        path: PathBuf::from(format!("/{path}")),
    }))
}

pub fn push_remote(
    local: &mut Repository,
    target: &RemoteTarget,
    ref_name: &str,
    unlock: &Unlock,
) -> Result<Value> {
    match target {
        RemoteTarget::Local(p) => push(local, p, ref_name, unlock),
        RemoteTarget::Http(h) => http_push(local, h, ref_name),
        RemoteTarget::Ssh(s) => ssh_sync(local, s, ref_name, unlock, true),
    }
}

pub fn pull_remote(
    local: &mut Repository,
    target: &RemoteTarget,
    ref_name: &str,
    unlock: &Unlock,
) -> Result<Value> {
    match target {
        RemoteTarget::Local(p) => pull(local, p, ref_name, unlock),
        RemoteTarget::Http(h) => http_pull(local, h, ref_name, unlock),
        RemoteTarget::Ssh(s) => ssh_sync(local, s, ref_name, unlock, false),
    }
}

fn pack_url(base: &str) -> String {
    format!("{}/wisp/v1/pack.zst", base.trim_end_matches('/'))
}

fn with_http_auth(mut req: ureq::Request) -> ureq::Request {
    if let Some(token) = http_bearer_token() {
        req = req.set("Authorization", &format!("Bearer {}", token));
    }
    req
}

fn http_push(local: &mut Repository, remote: &HttpRemote, ref_name: &str) -> Result<Value> {
    let body = pack_bytes(local)?;
    let url = pack_url(&remote.base);
    let resp = with_http_auth(ureq::put(&url).set("Content-Type", "application/zstd"))
        .send_bytes(&body)
        .map_err(|e| WispError::internal(format!("http push {url}: {e}")))?;
    if !(200..300).contains(&resp.status()) {
        return Err(WispError::internal(format!(
            "http push {}: status {}",
            url,
            resp.status()
        )));
    }
    let head = Ledger::head(local.ledger.conn(), ref_name)?
        .ok_or_else(|| WispError::usage("ref has no head"))?;
    Ok(json!({
        "direction": "push",
        "transport": "https",
        "url": url,
        "ref": ref_name,
        "head": head.to_string(),
        "bytes": body.len(),
    }))
}

fn http_pull(
    local: &mut Repository,
    remote: &HttpRemote,
    ref_name: &str,
    unlock: &Unlock,
) -> Result<Value> {
    let url = pack_url(&remote.base);
    let resp = with_http_auth(ureq::get(&url))
        .call()
        .map_err(|e| WispError::internal(format!("http pull {url}: {e}")))?;
    if resp.status() != 200 {
        return Err(WispError::internal(format!(
            "http pull {}: status {}",
            url,
            resp.status()
        )));
    }
    let mut body = Vec::new();
    resp.into_reader()
        .read_to_end(&mut body)
        .map_err(|e| WispError::internal(e.to_string()))?;
    apply_pack_bytes(local, &body, unlock)?;
    if Ledger::head(local.ledger.conn(), ref_name)?.is_some() {
        apply::rebuild_hot(local, ref_name)?;
    }
    let head = Ledger::head(local.ledger.conn(), ref_name)?;
    Ok(json!({
        "direction": "pull",
        "transport": "https",
        "url": url,
        "ref": ref_name,
        "head": head.map(|h| h.to_string()),
        "bytes": body.len(),
    }))
}

fn ssh_dest(s: &SshRemote) -> String {
    match &s.user {
        Some(u) => format!("{}@{}", u, s.host),
        None => s.host.clone(),
    }
}

/// Rsync the bundle directory to/from a remote path (requires `rsync` and `ssh` on PATH).
fn ssh_sync(
    local: &mut Repository,
    remote: &SshRemote,
    ref_name: &str,
    unlock: &Unlock,
    is_push: bool,
) -> Result<Value> {
    let staging = tempfile::tempdir()?;
    let bundle = staging.path().join("bundle");
    std::fs::create_dir_all(&bundle)?;
    let dest = ssh_dest(remote);
    let remote_spec = format!(
        "{}:{}/",
        dest,
        remote.path.to_string_lossy().trim_end_matches('/')
    );
    if is_push {
        init_trusted_replica(&bundle, &local.root, unlock)?;
        push(local, &bundle, ref_name, unlock)?;
        rsync(&bundle, &remote_spec, true)?;
        Ok(json!({
            "direction": "push",
            "transport": "ssh",
            "host": remote.host,
            "path": remote.path.display().to_string(),
            "ref": ref_name,
        }))
    } else {
        rsync(&bundle, &remote_spec, false)?;
        pull(local, &bundle, ref_name, unlock)?;
        Ok(json!({
            "direction": "pull",
            "transport": "ssh",
            "host": remote.host,
            "path": remote.path.display().to_string(),
            "ref": ref_name,
        }))
    }
}

fn rsync(local_bundle: &Path, remote_spec: &str, to_remote: bool) -> Result<()> {
    let mut cmd = Command::new("rsync");
    cmd.arg("-az").arg("-e").arg("ssh");
    if to_remote {
        cmd.arg(format!("{}/", local_bundle.display()))
            .arg(remote_spec);
    } else {
        cmd.arg(remote_spec)
            .arg(format!("{}/", local_bundle.display()));
    }
    let out = cmd
        .output()
        .map_err(|e| WispError::internal(format!("rsync: {e}")))?;
    if !out.status.success() {
        return Err(WispError::internal(format!(
            "rsync failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}
