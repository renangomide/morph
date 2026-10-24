// SPDX-License-Identifier: GPL-3.0-or-later
//! Memory budget for running tasks, so a batch of huge images can't run the
//! machine out of RAM (SPEC Â§10). Tasks are admitted only while the sum of
//! their estimates fits the budget; a task bigger than the whole budget runs
//! alone.

use crate::cancel::CancelToken;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// Bytes per pixel a typical image task needs at its peak: decoded RGBA8,
/// an oriented/resized copy, plus encoder buffers.
pub const IMAGE_BYTES_PER_PIXEL: u64 = 16;

/// Estimate when an input's size can't be read (the task will usually fail fast).
pub const UNKNOWN_ESTIMATE: u64 = 64 * 1024 * 1024;

/// Estimate for a task with PDF inputs: a few rendered pages (an A4 page at
/// 300 DPI is ~35 MB of RGBA) or re-encoded images, plus PDFium's caches.
pub const PDF_ESTIMATE: u64 = 256 * 1024 * 1024;

const MIN_BUDGET: u64 = 512 * 1024 * 1024;

/// Memory shared by every job of the process (two jobs at once must not
/// each take 60% of the RAM).
struct Pool {
    limit: u64,
    state: Mutex<State>,
    freed: Condvar,
}

#[derive(Default)]
struct State {
    used: u64,
    /// Running tasks across all jobs.
    running: usize,
}

static PROCESS_POOL: OnceLock<Arc<Pool>> = OnceLock::new();

/// Gate for one job's tasks: at most `slots` of them at once, and the memory
/// estimates of all running tasks (of every job using the same pool) must fit
/// the pool's limit.
pub struct Budget {
    pool: Arc<Pool>,
    slots: usize,
    /// This job's running tasks; only changed while holding `pool.state`.
    running: AtomicUsize,
}

/// Held by a running task; releases its share on drop (also on panic).
pub struct Permit<'a> {
    budget: &'a Budget,
    bytes: u64,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let pool = &self.budget.pool;
        if let Ok(mut s) = pool.state.lock() {
            s.used = s.used.saturating_sub(self.bytes);
            s.running = s.running.saturating_sub(1);
            let _ = self
                .budget
                .running
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        }
        pool.freed.notify_all();
    }
}

impl Budget {
    /// A budget with its own private `limit` (tests, CLI overrides).
    pub fn new(limit: u64, slots: usize) -> Self {
        Budget::with_pool(
            Arc::new(Pool {
                limit,
                state: Mutex::new(State::default()),
                freed: Condvar::new(),
            }),
            slots,
        )
    }

    /// The process-wide budget: 60% of the RAM that was free when the first
    /// job started (at least 512 MB).
    pub fn from_system(slots: usize) -> Self {
        let pool = PROCESS_POOL.get_or_init(|| {
            let limit = available_memory().map_or(2 * 1024 * 1024 * 1024, |a| a / 10 * 6);
            Arc::new(Pool {
                limit: limit.max(MIN_BUDGET),
                state: Mutex::new(State::default()),
                freed: Condvar::new(),
            })
        });
        Budget::with_pool(pool.clone(), slots)
    }

    fn with_pool(pool: Arc<Pool>, slots: usize) -> Self {
        Budget {
            pool,
            slots: slots.max(1),
            running: AtomicUsize::new(0),
        }
    }

    pub fn limit(&self) -> u64 {
        self.pool.limit
    }

    /// Wait until a task needing `bytes` may start. Returns `None` if the job
    /// is cancelled while it has to wait. A task bigger than the whole limit starts
    /// once nothing else is running.
    pub fn acquire(&self, bytes: u64, cancel: &CancelToken) -> Option<Permit<'_>> {
        let pool = &self.pool;
        let mut s = pool.state.lock().ok()?;
        loop {
            let fits = s.running == 0 || s.used.saturating_add(bytes) <= pool.limit;
            if self.running.load(Ordering::SeqCst) < self.slots && fits {
                s.used = s.used.saturating_add(bytes);
                s.running += 1;
                self.running.fetch_add(1, Ordering::SeqCst);
                return Some(Permit {
                    budget: self,
                    bytes,
                });
            }
            if cancel.is_cancelled() {
                return None;
            }
            // Timeout so a cancel is noticed even if nothing finishes.
            s = pool
                .freed
                .wait_timeout(s, Duration::from_millis(100))
                .ok()?
                .0;
        }
    }
}

/// Free physical memory in bytes.
pub fn available_memory() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a valid, initialised MEMORYSTATUSEX with dwLength set.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(status.ullAvailPhys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn respects_slots_and_memory() {
        let b = Budget::new(100, 3);
        let c = CancelToken::new();
        let p1 = b.acquire(60, &c).unwrap();
        // 60 + 60 > 100: must wait; cancel so the wait ends.
        let c2 = CancelToken::new();
        c2.cancel();
        assert!(b.acquire(60, &c2).is_none());
        let p2 = b.acquire(40, &c).unwrap();
        drop(p1);
        let p3 = b.acquire(60, &c).unwrap();
        drop((p2, p3));
    }

    #[test]
    fn oversized_task_runs_alone() {
        let b = Budget::new(100, 4);
        let c = CancelToken::new();
        let big = b.acquire(1_000, &c).unwrap();
        let c2 = CancelToken::new();
        c2.cancel();
        assert!(
            b.acquire(1, &c2).is_none(),
            "nothing joins an over-budget task"
        );
        drop(big);
        assert!(b.acquire(1, &c).is_some());
    }

    #[test]
    fn waiting_task_starts_when_memory_frees() {
        let b = Arc::new(Budget::new(100, 4));
        let c = CancelToken::new();
        let started = Arc::new(AtomicUsize::new(0));
        let p = b.acquire(80, &c).unwrap();
        let t = {
            let (b, c, started) = (b.clone(), c.clone(), started.clone());
            std::thread::spawn(move || {
                let _p = b.acquire(80, &c).unwrap();
                started.fetch_add(1, Ordering::SeqCst);
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(started.load(Ordering::SeqCst), 0);
        drop(p);
        t.join().unwrap();
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn jobs_share_the_process_budget() {
        let a = Budget::from_system(1);
        let b = Budget::from_system(1);
        let c = CancelToken::new();
        let big = a.acquire(a.limit(), &c).unwrap();
        let cancelled = CancelToken::new();
        cancelled.cancel();
        assert!(
            b.acquire(1, &cancelled).is_none(),
            "second job sees the first's memory"
        );
        drop(big);
        assert!(b.acquire(1, &c).is_some());
    }

    #[test]
    fn reads_system_memory() {
        assert!(available_memory().is_some_and(|m| m > 0));
    }
}
