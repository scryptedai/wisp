//! Single-threaded apply drain with optional same-actor coalesce.

use crate::headless::{ApplyBatch, ApplyOp};
use crate::manifest::RuntimeSection;
use crate::repo::{Repository, MAIN_REF};
use crate::runtime::cache::{AppendPatch, PublishEvent, SnapshotCache};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use wisp_model::{concept_path_of, SectionId, WispError};

/// Dedicated apply drain thread owned by the runtime store.
pub struct ApplyWriter {
    tx: Sender<ApplyJob>,
    join: Mutex<Option<JoinHandle<()>>>,
}

struct ApplyJob {
    batch: ApplyBatch,
    default_ref: Option<String>,
    deadline: Instant,
    reply: SyncSender<Result<Value, WispError>>,
    started: Arc<AtomicBool>,
}

#[derive(Clone)]
struct PendingUnit {
    batch: ApplyBatch,
    default_ref: Option<String>,
}

impl ApplyWriter {
    /// Start the writer loop on a background thread.
    pub fn spawn(
        repo: Arc<Mutex<Repository>>,
        config: RuntimeSection,
        cache: Arc<RwLock<SnapshotCache>>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("wisp-runtime-apply".into())
            .spawn(move || writer_loop(repo, config, cache, rx))
            .expect("runtime apply thread");
        ApplyWriter {
            tx,
            join: Mutex::new(Some(join)),
        }
    }

    /// Enqueue one apply batch and block until it completes or the deadline passes.
    pub fn submit(
        &self,
        batch: ApplyBatch,
        default_ref: Option<&str>,
        timeout_ms: u64,
    ) -> Result<Value, WispError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let started = Arc::new(AtomicBool::new(false));
        let job = ApplyJob {
            batch,
            default_ref: default_ref.map(|s| s.to_string()),
            deadline: Instant::now() + Duration::from_millis(timeout_ms.max(1)),
            reply: reply_tx,
            started: Arc::clone(&started),
        };
        self.tx
            .send(job)
            .map_err(|_| WispError::internal("runtime apply channel closed"))?;
        match reply_rx.recv_timeout(Duration::from_millis(timeout_ms.max(1))) {
            Ok(r) => r,
            Err(_) => {
                let retryable = !started.load(Ordering::Acquire);
                Err(WispError::runtime_timeout(
                    "runtime apply deadline elapsed",
                    json!({
                        "code": "runtime_timeout",
                        "retryable": retryable,
                        "outcome": if retryable { "queued" } else { "unknown" },
                    }),
                ))
            }
        }
    }
}

impl Drop for ApplyWriter {
    fn drop(&mut self) {
        let _ = self.join.lock().expect("apply join").take();
    }
}

fn writer_loop(
    repo: Arc<Mutex<Repository>>,
    config: RuntimeSection,
    cache: Arc<RwLock<SnapshotCache>>,
    rx: Receiver<ApplyJob>,
) {
    let window = Duration::from_micros(config.coalesce_window_us.max(1));
    while let Ok(first) = rx.recv() {
        let mut jobs = vec![first];
        let window_end = Instant::now() + window;
        while Instant::now() < window_end {
            match rx.try_recv() {
                Ok(j) => jobs.push(j),
                Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_micros(50)),
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        run_jobs(&repo, &config, &cache, &jobs);
    }
}

fn run_jobs(
    repo: &Arc<Mutex<Repository>>,
    config: &RuntimeSection,
    cache: &Arc<RwLock<SnapshotCache>>,
    jobs: &[ApplyJob],
) {
    let mut i = 0;
    while i < jobs.len() {
        if Instant::now() >= jobs[i].deadline {
            let _ = jobs[i].reply.send(Err(WispError::runtime_timeout(
                "runtime apply deadline elapsed before writer started job",
                json!({
                    "code": "runtime_timeout",
                    "retryable": true,
                    "outcome": "queued",
                }),
            )));
            i += 1;
            continue;
        }
        let (units, consumed) = if config.coalesce {
            coalesce_slice(&jobs[i..])
        } else {
            (
                vec![PendingUnit {
                    batch: jobs[i].batch.clone(),
                    default_ref: jobs[i].default_ref.clone(),
                }],
                1,
            )
        };
        let mut repo = match repo.lock() {
            Ok(g) => g,
            Err(_) => {
                let err = WispError::internal("runtime repo lock poisoned");
                for j in 0..consumed {
                    let _ = jobs[i + j].reply.send(Err(err.clone()));
                }
                i += consumed;
                continue;
            }
        };
        for j in 0..consumed {
            jobs[i + j].started.store(true, Ordering::Release);
        }
        match execute_unit(&mut repo, &units) {
            Ok(full) => {
                let touched: Vec<String> = units
                    .iter()
                    .flat_map(|u| u.batch.ops.iter().map(|o| o.path.clone()))
                    .collect();
                let default_ref = units
                    .first()
                    .and_then(|u| u.default_ref.clone())
                    .or_else(|| units.first().and_then(|u| u.batch.r#ref.clone()));
                let ref_name = default_ref.as_deref().unwrap_or(MAIN_REF);
                let patches = collect_append_patches(&units);
                let seq = {
                    let mut c = cache.write().expect("cache");
                    let seq = c.bump_seq();
                    if let Some(h) = full.get("head").and_then(|v| v.as_str()) {
                        c.set_head(ref_name, h);
                    }
                    let publish = if append_only_batch(&units) && !patches.is_empty() {
                        if c.try_patch_appends(ref_name, &patches, seq) {
                            c.invalidate_search_for_ref(ref_name);
                            patches
                                .iter()
                                .map(|p| PublishEvent::Append(p.clone()))
                                .collect()
                        } else {
                            c.invalidate_paths(ref_name, &touched);
                            c.invalidate_pack_prefix(ref_name);
                            vec![PublishEvent::Invalidate]
                        }
                    } else {
                        c.invalidate_paths(ref_name, &touched);
                        c.invalidate_pack_prefix(ref_name);
                        vec![PublishEvent::Invalidate]
                    };
                    c.record_publish(seq, publish);
                    seq
                };
                let mut out = full;
                if let Some(obj) = out.as_object_mut() {
                    obj.insert("seq".into(), json!(seq));
                }
                for j in 0..consumed {
                    let _ = jobs[i + j].reply.send(Ok(out.clone()));
                }
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

fn coalesce_slice(jobs: &[ApplyJob]) -> (Vec<PendingUnit>, usize) {
    if jobs.is_empty() {
        return (vec![], 0);
    }
    let first = &jobs[0];
    let first_actor = first.batch.actor.clone();
    let first_ref = effective_ref(first);
    let mut sections: HashSet<SectionId> = HashSet::new();
    let mut merged_ops: Vec<ApplyOp> = Vec::new();
    let mut consumed = 0usize;
    for job in jobs {
        if job.batch.ops.len() != 1 {
            break;
        }
        if !batches_coalesce_compatible(first, job) {
            break;
        }
        let op = &job.batch.ops[0];
        if !coalesce_op_eligible(op) {
            break;
        }
        if job.batch.actor != first_actor {
            break;
        }
        if effective_ref(job) != first_ref {
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
    )
}

fn effective_ref(job: &ApplyJob) -> String {
    job.batch
        .r#ref
        .clone()
        .or_else(|| job.default_ref.clone())
        .unwrap_or_else(|| MAIN_REF.to_string())
}

fn batches_coalesce_compatible(first: &ApplyJob, job: &ApplyJob) -> bool {
    first.batch.base == job.batch.base
        && first.batch.message == job.batch.message
        && first.batch.inputs == job.batch.inputs
        && first.batch.assert == job.batch.assert
        && first.batch.assert_head == job.batch.assert_head
        && first.batch.inputs_complete == job.batch.inputs_complete
        && first.default_ref == job.default_ref
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

fn append_only_batch(units: &[PendingUnit]) -> bool {
    units.iter().all(|u| {
        u.batch
            .ops
            .iter()
            .all(|o| matches!(o.op.as_str(), "append_section" | "prepend_section"))
    })
}

fn collect_append_patches(units: &[PendingUnit]) -> Vec<AppendPatch> {
    if !append_only_batch(units) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for u in units {
        for op in &u.batch.ops {
            let Some(section) = op.section else {
                return Vec::new();
            };
            let Some(content) = op.content.as_deref() else {
                continue;
            };
            out.push(AppendPatch {
                concept_path: concept_path_of(&op.path),
                section,
                fragment: content.to_string(),
                prepend: op.op == "prepend_section",
            });
        }
    }
    out
}
