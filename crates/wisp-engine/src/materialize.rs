//! Async materialize drain: hot → managed worktrees on a manifest heartbeat (docs/WORKTREE.md).

use crate::checkout;
use crate::publish::DirtySet;
use crate::repo::{Repository, Unlock};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use wisp_model::*;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct MaterializeStatus {
    pub pending_jobs: usize,
    pub pending_paths: usize,
    pub lag_ms: Option<u64>,
    pub last_drained_ms: Option<u64>,
    pub tick_ms: u64,
}

struct Shared {
    pending: BTreeMap<(PathBuf, String), DirtySet>,
    pending_paths: usize,
    oldest: Option<Instant>,
    last_drained: Option<Instant>,
    tick_ms: u64,
}

enum Command {
    Wake,
    DrainSync(mpsc::Sender<()>),
    Shutdown,
}

pub struct MaterializeQueue {
    tx: Sender<Command>,
    join: Mutex<Option<JoinHandle<()>>>,
    shared: Arc<Mutex<Shared>>,
}

impl MaterializeQueue {
    pub fn spawn(root: PathBuf, unlock: Unlock, tick_ms: u64) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            pending: BTreeMap::new(),
            pending_paths: 0,
            oldest: None,
            tick_ms,
            last_drained: None,
        }));
        let (tx, rx) = mpsc::channel();
        let tick = Duration::from_millis(tick_ms.max(1));
        let shared_worker = shared.clone();
        let join = thread::spawn(move || worker(root, unlock, tick, shared_worker, rx));
        Self {
            tx,
            join: Mutex::new(Some(join)),
            shared,
        }
    }

    pub fn status(&self) -> MaterializeStatus {
        let s = self.shared.lock().expect("materialize shared");
        MaterializeStatus {
            pending_jobs: s.pending.len(),
            pending_paths: s.pending_paths,
            lag_ms: s.oldest.map(|t| t.elapsed().as_millis() as u64),
            last_drained_ms: s.last_drained.map(|t| t.elapsed().as_millis() as u64),
            tick_ms: s.tick_ms,
        }
    }

    pub fn push_job(&self, root: PathBuf, ref_name: &str, dirty: DirtySet) {
        let n = dirty.concept_paths.len()
            + dirty.asset_paths.len()
            + dirty.prune_rels.len()
            + dirty.view_dirs.len();
        let mut s = self.shared.lock().expect("materialize shared");
        let key = (root, ref_name.to_string());
        match s.pending.get_mut(&key) {
            Some(existing) => existing.merge(&dirty),
            None => {
                s.pending.insert(key, dirty);
            }
        }
        s.pending_paths += n;
        if s.oldest.is_none() {
            s.oldest = Some(Instant::now());
        }
    }

    pub fn wake(&self) {
        let _ = self.tx.send(Command::Wake);
    }

    pub fn drain_blocking(&self, repo: &mut Repository) -> Result<()> {
        let (done_tx, done_rx) = mpsc::channel();
        let _ = self.tx.send(Command::DrainSync(done_tx));
        let _ = done_rx.recv();
        drain_pending_on_repo(repo, &self.shared)?;
        Ok(())
    }

    pub fn finish(&mut self, repo: &mut Repository) -> Result<()> {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(j) = self.join.lock().expect("materialize join").take() {
            let _ = j.join();
        }
        drain_pending_on_repo(repo, &self.shared)?;
        Ok(())
    }
}

fn drain_pending_on_repo(repo: &mut Repository, shared: &Arc<Mutex<Shared>>) -> Result<()> {
    let batch: Vec<((PathBuf, String), DirtySet)> = {
        let mut s = shared.lock().expect("materialize shared");
        let keys: Vec<(PathBuf, String)> = s.pending.keys().cloned().collect();
        let mut batch = Vec::with_capacity(keys.len());
        for k in keys {
            if let Some(v) = s.pending.remove(&k) {
                batch.push((k, v));
            }
        }
        s.pending_paths = 0;
        s.oldest = None;
        s.last_drained = Some(Instant::now());
        batch
    };
    for ((root, ref_name), dirty) in batch {
        let _ = checkout::publish_ref_delta(repo, &ref_name, &root, &dirty)?;
    }
    Ok(())
}

fn worker(
    root: PathBuf,
    unlock: Unlock,
    tick: Duration,
    shared: Arc<Mutex<Shared>>,
    rx: Receiver<Command>,
) {
    let mut repo = match Repository::open_worker(&root, &unlock) {
        Ok(r) => r,
        Err(_) => return,
    };
    loop {
        match rx.recv_timeout(tick) {
            Ok(Command::Shutdown) => break,
            Ok(Command::DrainSync(done)) => {
                let _ = drain_pending_on_repo(&mut repo, &shared);
                let _ = done.send(());
            }
            Ok(Command::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => {
                if shared.lock().expect("materialize shared").pending_paths > 0 {
                    let _ = drain_pending_on_repo(&mut repo, &shared);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = drain_pending_on_repo(&mut repo, &shared);
}
