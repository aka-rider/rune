use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

use super::{Cmd, Msg};

const DEFAULT_SIZE: usize = 4;
const MIN_SIZE: usize = 2;
const MAX_SIZE: usize = 8;

pub(crate) fn size() -> usize {
    thread::available_parallelism()
        .map_or(DEFAULT_SIZE, |n| n.get())
        .clamp(MIN_SIZE, MAX_SIZE)
}

// `catch_unwind` guards a `Cmd`'s own Rust panic; a crash in linked C
// aborts the process without unwinding.
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
