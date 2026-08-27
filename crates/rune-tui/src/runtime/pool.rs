use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

use super::{Cmd, Msg};

const DEFAULT_SIZE: usize = 4;
const MIN_SIZE: usize = 2;
const MAX_SIZE: usize = 8;

/// How many worker threads [`Pool::new`] should spawn: the machine's own
/// parallelism, clamped to a band sane for a TUI's background work — never
/// below [`MIN_SIZE`] (a single-core report should still overlap two
/// `Cmd`s), never above [`MAX_SIZE`] (a many-core build box has no reason
/// to hold open more idle OS threads than this app ever has concurrent
/// off-thread work for).
pub(crate) fn size() -> usize {
    thread::available_parallelism()
        .map_or(DEFAULT_SIZE, |n| n.get())
        .clamp(MIN_SIZE, MAX_SIZE)
}

/// Runs `cmd` to completion and forwards its reply, if any, to `tx` —
/// exactly what `spawn_cmd` used to do inline in its own `thread::spawn`
/// closure, now shared by every pool worker's loop body and by the
/// dedicated per-save thread `spawn_cmd` still spawns.  `catch_unwind`
/// guards a `Cmd`'s own Rust panic; it cannot and does not guard a crash in
/// linked C, which aborts the process rather than unwinding it.
pub(crate) fn run_and_reply(cmd: Cmd, tx: &mpsc::Sender<Msg>) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cmd.run())) {
        Ok(Some(msg)) => {
            let _ = tx.send(msg);
        }
        Ok(None) => {}
        Err(_) => {
            let _ = tx.send(Msg::Posted {
                severity: crate::messages::Severity::Error,
                text: "a background task panicked".to_string(),
            });
        }
    }
}

/// A fixed-size worker pool for every `Cmd` kind except `Save` (which keeps
/// its own dedicated, joinable thread per publish — see `spawn_cmd` — so
/// `exit_settle::join_save_handles` can keep waiting on exactly the set of
/// in-flight publishes it always has). One dedicated channel per worker
/// (round-robin dispatch) rather than a single channel behind a shared
/// `Mutex`: it needs no lock and no poison-recovery path to stay panic-safe.
pub(crate) struct Pool {
    senders: Vec<mpsc::Sender<Cmd>>,
    next: AtomicUsize,
}

impl Pool {
    pub(crate) fn new(size: usize, reply_tx: mpsc::Sender<Msg>) -> Pool {
        let senders = (0..size.max(1))
            .map(|_| {
                let (tx, rx) = mpsc::channel::<Cmd>();
                let reply_tx = reply_tx.clone();
                thread::spawn(move || {
                    while let Ok(cmd) = rx.recv() {
                        run_and_reply(cmd, &reply_tx);
                    }
                });
                tx
            })
            .collect();
        Pool {
            senders,
            next: AtomicUsize::new(0),
        }
    }

    /// Hands `cmd` to whichever worker is next in the round-robin —
    /// bounded total threads, unbounded queue depth per worker. Ordering
    /// across workers was never guaranteed even under the old one-thread-
    /// per-`Cmd` scheme (the OS scheduler alone decided who finished
    /// first), so this changes nothing a caller could have relied on: every
    /// reply that must survive reordering already carries its own
    /// generation or version echo.
    pub(crate) fn submit(&self, cmd: Cmd) {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.senders.len().max(1);
        if let Some(sender) = self.senders.get(idx) {
            let _ = sender.send(cmd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_cmds_than_workers_all_complete() {
        let (tx, rx) = mpsc::channel::<Msg>();
        let pool = Pool::new(2, tx);
        let n = 10;
        for i in 0..n {
            pool.submit(Cmd::read_dir(move || {
                Some(Msg::Posted {
                    severity: crate::messages::Severity::Info,
                    text: i.to_string(),
                })
            }));
        }

        let mut seen = Vec::new();
        for _ in 0..n {
            if let Ok(Msg::Posted { text, .. }) = rx.recv() {
                seen.push(text);
            }
        }
        seen.sort();

        let mut expected: Vec<String> = (0..n).map(|i| i.to_string()).collect();
        expected.sort();
        assert_eq!(seen, expected, "every submitted Cmd's reply must arrive");
    }

    #[test]
    fn size_never_falls_outside_its_band() {
        let n = size();
        assert!((MIN_SIZE..=MAX_SIZE).contains(&n));
    }
}
