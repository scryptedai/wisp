//! Push/pull a cold pack over a subprocess running `wisp serve --stdio`.

use crate::stdio::{client_pull_cold_pack, client_push_cold_pack};
use serde_json::{json, Value};
use std::io::{BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use wisp_engine::repo::{Repository, Unlock};
use wisp_model::*;

#[derive(Clone, Debug)]
pub struct StdioRemote {
    /// Shell command (e.g. `ssh user@host wisp serve --stdio`).
    pub shell: String,
}

/// Run `shell` as a subprocess; its stdin/stdout carry the JSON-lines ColdPack session.
pub fn stdio_push(local: &Repository, ref_name: &str, remote: &StdioRemote) -> Result<Value> {
    let push = stdio_session(remote, |input, output| {
        client_push_cold_pack(local, ref_name, input, output)
    })?;
    Ok(json!({
        "direction": "push",
        "transport": "stdio",
        "mode": "cold_pack",
        "ref": ref_name,
        "command": remote.shell,
        "push": push,
    }))
}

/// Pull a cold pack from `remote.shell` (subprocess running `wisp serve --stdio`).
pub fn stdio_pull(
    local: &mut Repository,
    ref_name: &str,
    remote: &StdioRemote,
    unlock: &Unlock,
) -> Result<Value> {
    let pull = stdio_session(remote, |input, output| {
        client_pull_cold_pack(local, unlock, ref_name, input, output)
    })?;
    Ok(json!({
        "direction": "pull",
        "transport": "stdio",
        "mode": "cold_pack_pull",
        "ref": ref_name,
        "command": remote.shell,
        "pull": pull,
    }))
}

fn stdio_session<R>(
    remote: &StdioRemote,
    run: impl FnOnce(&mut BufReader<ChildStdout>, &mut BufWriter<ChildStdin>) -> Result<R>,
) -> Result<R> {
    let mut child = spawn_shell(&remote.shell)?;
    child.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = child
        .spawn()
        .map_err(|e| WispError::internal(format!("stdio sync spawn: {e}")))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| WispError::internal("stdio sync: child stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WispError::internal("stdio sync: child stdout"))?;
    let mut input = BufReader::new(stdout);
    let mut output = BufWriter::new(stdin);
    let exchange = run(&mut input, &mut output);
    let flush_err = output.flush();
    drop(output);
    drop(input);

    if let Err(e) = flush_err {
        terminate_child(&mut child);
        return Err(WispError::internal(e.to_string()));
    }
    let result = match exchange {
        Err(e) => {
            terminate_child(&mut child);
            return Err(e);
        }
        Ok(r) => r,
    };
    let status = child
        .wait()
        .map_err(|e| WispError::internal(format!("stdio sync wait: {e}")))?;
    if !status.success() {
        return Err(WispError::internal(format!(
            "stdio sync command exited with {status}"
        )));
    }
    Ok(result)
}

/// Stop and reap the shell child after a failed protocol exchange (pipes are already closed).
fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
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
