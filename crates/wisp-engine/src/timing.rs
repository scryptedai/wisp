//! Optional phase timings for development profiling (`wisp_engine::timing::with_profile`).

use std::cell::RefCell;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct PhaseSample {
    pub name: &'static str,
    pub nanos: u128,
}

thread_local! {
    static PROFILER: RefCell<Option<Profiler>> = const { RefCell::new(None) };
}

struct Profiler {
    phases: Vec<PhaseSample>,
    stack: Vec<(Instant, &'static str)>,
}

impl Profiler {
    fn push(name: &'static str) {
        PROFILER.with(|cell| {
            if let Some(p) = cell.borrow_mut().as_mut() {
                p.stack.push((Instant::now(), name));
            }
        });
    }

    fn pop() {
        PROFILER.with(|cell| {
            if let Some(p) = cell.borrow_mut().as_mut() {
                if let Some((t0, name)) = p.stack.pop() {
                    p.phases.push(PhaseSample {
                        name,
                        nanos: t0.elapsed().as_nanos(),
                    });
                }
            }
        });
    }
}

/// Run `f` while recording `phase!` spans; returns `(result, phases)`.
pub fn with_profile<R>(f: impl FnOnce() -> R) -> (R, Vec<PhaseSample>) {
    PROFILER.with(|cell| {
        *cell.borrow_mut() = Some(Profiler {
            phases: Vec::new(),
            stack: Vec::new(),
        });
    });
    let out = f();
    let phases = PROFILER.with(|cell| {
        let mut guard = cell.borrow_mut();
        let p = guard.take().expect("profiler");
        p.phases
    });
    (out, phases)
}

/// Record elapsed time for a synchronous block when profiling is active.
pub fn phase<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    Profiler::push(name);
    let r = f();
    Profiler::pop();
    r
}

/// Mark a point phase (push+pop around zero work) — use `phase(name, || { ... })` instead.
#[macro_export]
macro_rules! profile_scope {
    ($name:expr, $body:expr) => {
        $crate::timing::phase($name, || $body)
    };
}
