//! Push a cold pack over a subprocess speaking `wisp serve --stdio` (ColdPack mode).

use crate::stdio::client_push_cold_pack;
use serde_json::{json, Value};
use std::io::{BufReader, BufWriter, Write};
use std::process::{Command, Stdio};
use wisp_engine::repo::Repository;
use wisp_model::*;

#[derive(Clone, Debug)]
pub struct StdioRemote {
    /// Shell command (e.g. `ssh user@host wisp serve --stdio`).
    pub shell: String,
}

/// Run `shell` as a subprocess; its stdin/stdout carry the JSON-lines ColdPack session.
pub fn stdio_push(local: &Repository, ref_name: &str, remote: &StdioRemote) -> Result<Value> {
    let mut child = spawn_shell(&remote.shell)?;
    child.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = child
        .spawn()
        .map_err(|e| WispError::internal(format!("stdio push spawn: {e}")))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| WispError::internal("stdio push: child stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WispError::internal("stdio push: child stdout"))?;
    let mut input = BufReader::new(stdout);
    let mut output = BufWriter::new(stdin);
    let push = client_push_cold_pack(local, ref_name, &mut input, &mut output)?;
    output
        .flush()
        .map_err(|e| WispError::internal(e.to_string()))?;
    let status = child
        .wait()
        .map_err(|e| WispError::internal(format!("stdio push wait: {e}")))?;
    if !status.success() {
        return Err(WispError::internal(format!(
            "stdio push command exited with {status}"
        )));
    }
    Ok(json!({
        "direction": "push",
        "transport": "stdio",
        "mode": "cold_pack",
        "ref": ref_name,
        "command": remote.shell,
        "push": push,
    }))
}

fn spawn_shell(shell: &str) -> Result<Command> {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(shell);
        Ok(cmd)
    } else {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(shell);
        Ok(cmd)
    }
}
