//! Bundle-local commit queue: agents submit `ApplyBatch` jobs and await results; one writer
//! owns `ledger.db`. Optional coalescing merges disjoint eligible ops in a drain window.

use crate::headless::{ApplyBatch, ApplyOp};
use crate::manifest::CommitQueueSection;
use crate::repo::{Repository, Unlock};
#[cfg(unix)]
use crate::repo::MAIN_REF;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use wisp_model::{ExitCode, SectionId, WispError};

enum QueueMsg {
    Job(Box<Job>),
    Stop,
}

#[derive(Debug)]
struct Job {
    batch: ApplyBatch,
    default_ref: Option<String>,
    queue_deadline: Instant,
    reply: Sender<Result<Value, WispError>>,
}

/// Cumulative counters for the commit-queue writer (jobs, commits, coalesce merges).
#[derive(Debug, Clone, Default)]
pub struct QueueStats {
    pub jobs_submitted: u64,
    pub jobs_completed: u64,
    pub jobs_failed: u64,
    pub queue_timeouts: u64,
    pub writer_commits: u64,
    pub ops_committed: u64,
    pub coalesce_merges: u64,
}

#[derive(Debug, Default)]
struct Shared {
    stats: QueueStats,
}

/// In-process commit queue (tests and embedded). Use [`CommitQueue::bind_unix`] for CLI agents.
pub struct CommitQueue {
    tx: Mutex<Option<Sender<QueueMsg>>>,
    join: Mutex<Option<JoinHandle<()>>>,
    shared: Arc<Mutex<Shared>>,
}

impl CommitQueue {
    /// Start the writer on a worker `Repository` (no per-client cold-flush threads).
    pub fn spawn(
        root: PathBuf,
        unlock: Unlock,
        config: CommitQueueSection,
    ) -> Result<Arc<Self>, WispError> {
        let (tx, rx) = mpsc::channel::<QueueMsg>();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let cfg = config.clone();
        let shared_worker = shared.clone();
        let join = thread::Builder::new()
            .name("wisp-commit-queue".into())
            .spawn(move || writer_loop(root, unlock, cfg, rx, shared_worker))
            .map_err(|e| WispError::internal(format!("commit queue thread: {e}")))?;
        Ok(Arc::new(CommitQueue {
            tx: Mutex::new(Some(tx)),
            join: Mutex::new(Some(join)),
            shared,
        }))
    }

    /// Snapshot of [`QueueStats`] since this queue was spawned.
    pub fn stats(&self) -> QueueStats {
        self.shared.lock().expect("queue stats").stats.clone()
    }

    /// Submit and block until the writer finishes or the queue deadline elapses.
    pub fn submit(
        &self,
        batch: ApplyBatch,
        default_ref: Option<&str>,
        queue_timeout: Duration,
    ) -> Result<Value, WispError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let job = Job {
            batch,
            default_ref: default_ref.map(|s| s.to_string()),
            queue_deadline: Instant::now() + queue_timeout,
            reply: reply_tx,
        };
        self.shared
            .lock()
            .expect("queue stats")
            .stats
            .jobs_submitted += 1;
        let tx = self
            .tx
            .lock()
            .expect("queue tx")
            .as_ref()
            .ok_or_else(|| WispError::internal("commit queue stopped"))?
            .clone();
        tx.send(QueueMsg::Job(Box::new(job)))
            .map_err(|_| WispError::internal("commit queue stopped"))?;
        match reply_rx.recv_timeout(queue_timeout) {
            Ok(r) => {
                let mut st = self.shared.lock().expect("queue stats");
                match &r {
                    Ok(_) => st.stats.jobs_completed += 1,
                    Err(e) if e.code == ExitCode::QueueTimeout => st.stats.queue_timeouts += 1,
                    Err(_) => st.stats.jobs_failed += 1,
                }
                r
            }
            Err(_) => {
                let mut st = self.shared.lock().expect("queue stats");
                st.stats.queue_timeouts += 1;
                Err(WispError::queue_timeout(
                    "commit queue deadline elapsed before the job completed",
                    json!({
                        "retryable": true,
                        "phase": "queue.submit_wait",
                        "queue_timeout_ms": queue_timeout.as_millis(),
                    }),
                ))
            }
        }
    }

    /// Stop the writer thread and drain the queue (idempotent).
    pub fn shutdown(&self) {
        if let Some(tx) = self.tx.lock().expect("queue tx").take() {
            let _ = tx.send(QueueMsg::Stop);
        }
        if let Some(j) = self.join.lock().expect("join").take() {
            let _ = j.join();
        }
    }
}

fn writer_loop(
    root: PathBuf,
    unlock: Unlock,
    config: CommitQueueSection,
    rx: Receiver<QueueMsg>,
    shared: Arc<Mutex<Shared>>,
) {
    let mut repo = match Repository::open_worker(&root, &unlock) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("commit queue: open failed: {e}");
            drain_fail_all(rx, e);
            return;
        }
    };
    let coalesce_window = Duration::from_micros(config.coalesce_window_us.max(1));
    while let Ok(msg) = rx.recv() {
        match msg {
            QueueMsg::Stop => break,
            QueueMsg::Job(first) => {
                let mut jobs = vec![first];
                let window_end = Instant::now() + coalesce_window;
                while Instant::now() < window_end {
                    match rx.try_recv() {
                        Ok(QueueMsg::Job(j)) => jobs.push(j),
                        Ok(QueueMsg::Stop) => {
                            run_jobs(&mut repo, &config, &jobs, &shared);
                            return;
                        }
                        Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_micros(50)),
                        Err(mpsc::TryRecvError::Disconnected) => break,
                    }
                }
                run_jobs(&mut repo, &config, &jobs, &shared);
            }
        }
    }
}

fn drain_fail_all(rx: Receiver<QueueMsg>, err: WispError) {
    while let Ok(msg) = rx.recv() {
        if let QueueMsg::Job(job) = msg {
            let _ = job.reply.send(Err(err.clone()));
        }
    }
}

fn run_jobs(
    repo: &mut Repository,
    config: &CommitQueueSection,
    jobs: &[Box<Job>],
    shared: &Arc<Mutex<Shared>>,
) {
    let mut i = 0;
    while i < jobs.len() {
        if Instant::now() >= jobs[i].queue_deadline {
            let _ = jobs[i].reply.send(Err(WispError::queue_timeout(
                "job expired before the writer started it",
                json!({
                    "retryable": true,
                    "phase": "queue.writer_dequeue",
                }),
            )));
            i += 1;
            continue;
        }
        let (units, consumed, merged) = if config.coalesce {
            coalesce_slice(&jobs[i..])
        } else {
            (
                vec![PendingUnit {
                    batch: jobs[i].batch.clone(),
                    default_ref: jobs[i].default_ref.clone(),
                }],
                1,
                false,
            )
        };
        if merged {
            shared.lock().expect("stats").stats.coalesce_merges += 1;
        }
        let result = execute_unit(repo, &units);
        match result {
            Ok(full) => {
                for j in 0..consumed {
                    let reply = reply_for_job(&full, &jobs[i + j].batch);
                    let _ = jobs[i + j].reply.send(Ok(reply));
                }
                record_success(shared, &units);
            }
            Err(e) => {
                for j in 0..consumed {
                    let _ = jobs[i + j].reply.send(Err(e.clone()));
                }
            }
        }
        i += consumed;
    }
}

fn record_success(shared: &Arc<Mutex<Shared>>, unit: &[PendingUnit]) {
    let mut st = shared.lock().expect("stats");
    st.stats.writer_commits += 1;
    st.stats.ops_committed += unit.iter().map(|u| u.batch.ops.len() as u64).sum::<u64>();
}

#[derive(Clone)]
struct PendingUnit {
    batch: ApplyBatch,
    default_ref: Option<String>,
}

fn coalesce_slice(jobs: &[Box<Job>]) -> (Vec<PendingUnit>, usize, bool) {
    if jobs.is_empty() {
        return (vec![], 0, false);
    }
    let first_actor = jobs[0].batch.actor.clone();
    let mut sections: HashSet<SectionId> = HashSet::new();
    let mut merged_ops: Vec<ApplyOp> = Vec::new();
    let mut consumed = 0usize;
    for job in jobs {
        if job.batch.ops.len() != 1 {
            break;
        }
        let op = &job.batch.ops[0];
        if !coalesce_op_eligible(op) {
            break;
        }
        if job.batch.actor != first_actor {
            break;
        }
        let sec = match op.section {
            Some(s) => s,
            None => break,
        };
        if sections.contains(&sec) {
            break;
        }
        sections.insert(sec);
        merged_ops.push(op.clone());
        consumed += 1;
    }
    if consumed <= 1 {
        return (
            vec![PendingUnit {
                batch: jobs[0].batch.clone(),
                default_ref: jobs[0].default_ref.clone(),
            }],
            1,
            false,
        );
    }
    let mut batch = jobs[0].batch.clone();
    batch.ops = merged_ops;
    (
        vec![PendingUnit {
            batch,
            default_ref: jobs[0].default_ref.clone(),
        }],
        consumed,
        true,
    )
}

fn coalesce_op_eligible(op: &ApplyOp) -> bool {
    matches!(
        op.op.as_str(),
        "update_section" | "append_section" | "prepend_section"
    )
}

fn execute_unit(repo: &mut Repository, unit: &[PendingUnit]) -> Result<Value, WispError> {
    let mut last = json!({});
    for u in unit {
        last = crate::headless::apply_direct(repo, u.batch.clone(), u.default_ref.as_deref())?;
    }
    Ok(last)
}

fn reply_for_job(full: &Value, batch: &ApplyBatch) -> Value {
    if batch.ops.len() != 1 {
        return full.clone();
    }
    let want = batch.ops[0].section.map(|s| s.to_string());
    let changes = full
        .get("changes")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let filtered: Vec<Value> = changes
        .into_iter()
        .filter(|c| {
            want.as_ref()
                .is_none_or(|w| c.get("section").and_then(|x| x.as_str()) == Some(w.as_str()))
        })
        .collect();
    let mut out = full.clone();
    out["changes"] = json!(filtered);
    out
}

impl CommitQueue {
    /// Unix domain socket at `wisp_dir/commit.sock` (one JSON line per request).
    #[cfg(unix)]
    pub fn bind_unix(
        wisp_dir: &Path,
        unlock: Unlock,
        config: CommitQueueSection,
    ) -> Result<Arc<Self>, WispError> {
        let root = wisp_dir
            .parent()
            .ok_or_else(|| WispError::internal("wisp_dir has no parent"))?;
        let queue = Self::spawn(root.to_path_buf(), unlock, config)?;
        let listener_path = wisp_dir.join("commit.sock");
        if listener_path.exists() {
            let _ = std::fs::remove_file(&listener_path);
        }
        let listener = std::os::unix::net::UnixListener::bind(&listener_path)
            .map_err(|e| WispError::internal(format!("bind commit.sock: {e}")))?;
        let q = queue.clone();
        thread::Builder::new()
            .name("wisp-commit-queue-accept".into())
            .spawn(move || accept_loop(listener, q))
            .map_err(|e| WispError::internal(format!("accept thread: {e}")))?;
        Ok(queue)
    }

    /// Commit-queue socket transport is Unix-only; use in-process [`CommitQueue::spawn`] on other OSes.
    #[cfg(not(unix))]
    pub fn bind_unix(
        _wisp_dir: &Path,
        _unlock: Unlock,
        _config: CommitQueueSection,
    ) -> Result<Arc<Self>, WispError> {
        Err(WispError::usage(
            "commit queue socket (`wisp queue --serve`) is not supported on this platform; \
             use Unix or disable queue.enabled and commit directly",
        ))
    }
}

#[cfg(unix)]
fn accept_loop(listener: std::os::unix::net::UnixListener, queue: Arc<CommitQueue>) {
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let q = queue.clone();
                thread::spawn(move || handle_client(s, q));
            }
            Err(e) => eprintln!("commit queue accept: {e}"),
        }
    }
}

#[cfg(unix)]
fn handle_client(stream: std::os::unix::net::UnixStream, queue: Arc<CommitQueue>) {
    use std::io::{BufRead, BufReader, Write};
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut stream = reader.into_inner();
    let resp = match serde_json::from_str::<Value>(&line) {
        Ok(v) => {
            let batch_val = v.get("batch").cloned().unwrap_or(Value::Null);
            match serde_json::from_value::<ApplyBatch>(batch_val) {
                Ok(batch) => {
                    let ms = v
                        .get("timeout_ms")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(120_000);
                    queue.submit(batch, Some(MAIN_REF), Duration::from_millis(ms))
                }
                Err(e) => Err(WispError::usage(format!("commit.sock batch: {e}"))),
            }
        }
        Err(e) => Err(WispError::usage(format!("commit.sock json: {e}"))),
    };
    let out = match resp {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(e) => json!({ "ok": false, "error": e }),
    };
    let _ = writeln!(
        stream,
        "{}",
        serde_json::to_string(&out).unwrap_or_default()
    );
}

/// Client: one JSON line to `.wisp/commit.sock` (see `handle_client`).
#[cfg(unix)]
pub fn submit_via_socket(
    wisp_dir: &Path,
    batch: ApplyBatch,
    default_ref: Option<&str>,
    queue_timeout: Duration,
) -> Result<Value, WispError> {
    let sock = wisp_dir.join("commit.sock");
    let mut stream = std::os::unix::net::UnixStream::connect(&sock).map_err(|e| {
        WispError::not_found(format!(
            "commit queue socket ({}): {e}; start a writer with `wisp queue --serve`",
            sock.display()
        ))
    })?;
    let req = json!({
        "batch": batch,
        "default_ref": default_ref,
        "timeout_ms": queue_timeout.as_millis(),
    });
    use std::io::{BufRead, BufReader, Write};
    writeln!(stream, "{}", serde_json::to_string(&req).unwrap())
        .map_err(|e| WispError::internal(format!("commit.sock write: {e}")))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| WispError::internal(format!("commit.sock read: {e}")))?;
    let v: Value = serde_json::from_str(&line)
        .map_err(|e| WispError::internal(format!("commit.sock response: {e}")))?;
    if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        Ok(v.get("data").cloned().unwrap_or(Value::Null))
    } else if let Some(err) = v.get("error") {
        serde_json::from_value(err.clone()).map_err(|e| WispError::internal(format!("{e}")))
    } else {
        Err(WispError::internal("commit.sock: malformed response"))
    }
}

#[cfg(not(unix))]
pub fn submit_via_socket(
    _wisp_dir: &Path,
    _batch: ApplyBatch,
    _default_ref: Option<&str>,
    _queue_timeout: Duration,
) -> Result<Value, WispError> {
    Err(WispError::usage(
        "commit queue socket is not supported on this platform; disable queue.enabled or use Unix",
    ))
}
