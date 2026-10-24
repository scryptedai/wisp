//! Background drain of `staged_payloads` to `.wisp/payloads/` (§10.2 step 3).
//!
//! Commits stage ciphertext in the ledger and wake this queue; fsync to cold files runs
//! off the hot path. Call [`Repository::flush_cold_payloads`] at sync/shutdown barriers.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use wisp_model::Result;
use wisp_store::Ledger;

/// Default max staged ciphertext backlog before commit path blocks to drain (~128 MiB).
const DEFAULT_MAX_STAGED_BYTES: u64 = 128 * 1024 * 1024;

fn max_staged_bytes() -> u64 {
    std::env::var("WISP_MAX_STAGED_PAYLOAD_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_STAGED_BYTES)
}

enum Command {
    Wake,
    DrainSync(mpsc::Sender<()>),
    Shutdown,
}

pub struct ColdFlushQueue {
    tx: Sender<Command>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl ColdFlushQueue {
    pub fn spawn(wisp_dir: PathBuf, busy_timeout_ms: u64) -> Self {
        let (tx, rx) = mpsc::channel();
        let join = thread::spawn(move || worker(wisp_dir, busy_timeout_ms, rx));
        Self {
            tx,
            join: Mutex::new(Some(join)),
        }
    }

    /// After a successful commit that staged new payloads.
    pub fn wake(&self) {
        let _ = self.tx.send(Command::Wake);
    }

    /// Block until staged rows are flushed to payload files (sync / shutdown barrier).
    pub fn drain_blocking(&self, ledger: &Ledger) -> Result<()> {
        self.maybe_backpressure(ledger)?;
        let (done_tx, done_rx) = mpsc::channel();
        let _ = self.tx.send(Command::DrainSync(done_tx));
        let _ = done_rx.recv();
        drain_on_connection(ledger)?;
        Ok(())
    }

    /// If staged backlog exceeds the configured limit, drain on this connection until under max.
    pub fn maybe_backpressure(&self, ledger: &Ledger) -> Result<()> {
        let max = max_staged_bytes();
        while Ledger::staged_backlog_bytes(ledger.conn())? > max {
            if ledger.flush_staged()? == 0 {
                break;
            }
        }
        Ok(())
    }

    pub fn shutdown(&mut self, ledger: &Ledger) -> Result<()> {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(j) = self.join.lock().expect("cold flush join").take() {
            let _ = j.join();
        }
        drain_on_connection(ledger)?;
        Ok(())
    }
}

fn drain_on_connection(ledger: &Ledger) -> Result<()> {
    while ledger.flush_staged()? > 0 {}
    Ok(())
}

fn worker(wisp_dir: PathBuf, busy_timeout_ms: u64, rx: Receiver<Command>) {
    let ledger = match Ledger::open(&wisp_dir, busy_timeout_ms) {
        Ok(l) => l,
        Err(_) => return,
    };
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Wake => drain_worker(&ledger),
            Command::DrainSync(done) => {
                drain_worker(&ledger);
                let _ = done.send(());
            }
            Command::Shutdown => {
                drain_worker(&ledger);
                break;
            }
        }
    }
}

fn drain_worker(ledger: &Ledger) {
    while ledger.flush_staged().unwrap_or(0) > 0 {}
}
