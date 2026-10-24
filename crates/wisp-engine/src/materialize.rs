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

/// Backlog snapshot for `wisp status --json` and `Repository::materialize_status`.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct MaterializeStatus {
    /// Coalesced `(tree_root, ref)` jobs waiting for the worker.
    pub pending_jobs: usize,
    /// Sum of dirty paths across queued jobs (approximate work units).
    pub pending_paths: usize,
    /// Milliseconds since the oldest queued job was enqueued.
    pub lag_ms: Option<u64>,
    /// Milliseconds since the worker last drained the queue.
    pub last_drained_ms: Option<u64>,
    /// Manifest `materialize.tick_ms` at queue creation.
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

/// Background hot→disk publish queue for managed worktrees (docs/WORKTREE.md).
pub struct MaterializeQueue {
    tx: Sender<Command>,
    join: Mutex<Option<JoinHandle<()>>>,
    shared: Arc<Mutex<Shared>>,
}

impl MaterializeQueue {
    /// Start the heartbeat worker for `root` (one queue per `Repository` handle).
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

    /// Current backlog metrics (lock-free aside from shared mutex).
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

    /// Enqueue or merge a publish set for `(root, ref_name)`.
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

    /// Nudge the worker to drain before the next tick.
    pub fn wake(&self) {
        let _ = self.tx.send(Command::Wake);
    }

    /// Barrier: wait for the worker, then publish any jobs still on the shared queue.
    pub fn drain_blocking(&self, repo: &mut Repository) -> Result<()> {
        let (done_tx, done_rx) = mpsc::channel();
        let _ = self.tx.send(Command::DrainSync(done_tx));
        let _ = done_rx.recv();
        drain_pending_on_repo(repo, &self.shared)?;
        Ok(())
    }

    /// Shut down the worker and drain remaining jobs (repository drop).
    pub fn finish(&mut self, repo: &mut Repository) -> Result<()> {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(j) = self.join.lock().expect("materialize join").take() {
            let _ = j.join();
        }
        drain_pending_on_repo(repo, &self.shared)?;
        Ok(())
    }
}

fn dirty_path_count(dirty: &DirtySet) -> usize {
    dirty.concept_paths.len()
        + dirty.asset_paths.len()
        + dirty.prune_rels.len()
        + dirty.view_dirs.len()
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
    let mut first_err = None;
    for ((root, ref_name), dirty) in batch {
        if let Err(e) = checkout::publish_ref_delta(repo, &ref_name, &root, &dirty) {
            let n = dirty_path_count(&dirty);
            let mut s = shared.lock().expect("materialize shared");
            s.pending_paths += n;
            s.oldest.get_or_insert_with(Instant::now);
            s.pending
                .entry((root, ref_name))
                .and_modify(|d| d.merge(&dirty))
                .or_insert(dirty);
            first_err.get_or_insert(e);
        }
    }
    first_err.map_or(Ok(()), Err)
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
        Err(e) => {
            eprintln!(
                "materialize worker: failed to open repository at {}: {e}",
                root.display()
            );
            return;
        }
    };
    loop {
        match rx.recv_timeout(tick) {
            Ok(Command::Shutdown) => break,
            Ok(Command::DrainSync(done)) => {
                if let Err(e) = drain_pending_on_repo(&mut repo, &shared) {
                    eprintln!("materialize worker: drain failed: {e}");
                }
                let _ = done.send(());
            }
            Ok(Command::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => {
                if shared.lock().expect("materialize shared").pending_paths > 0 {
                    if let Err(e) = drain_pending_on_repo(&mut repo, &shared) {
                        eprintln!("materialize worker: drain failed: {e}");
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = drain_pending_on_repo(&mut repo, &shared);
}
