use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::document::DocumentId;

use super::Msg;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimerKey {
    Snapshot(DocumentId),
    SaveConfirm,
    QuitConfirm,
    MessagesCollapse,
}

struct TimerState {
    pending: HashMap<TimerKey, (Instant, Msg)>,
    tx: Option<Sender<Msg>>,
    thread_spawned: bool,
}

pub struct TimerService {
    state: Mutex<TimerState>,
    condvar: Condvar,
}

impl TimerService {
    pub fn new() -> Arc<TimerService> {
        Arc::new(TimerService {
            state: Mutex::new(TimerState {
                pending: HashMap::new(),
                tx: None,
                thread_spawned: false,
            }),
            condvar: Condvar::new(),
        })
    }

    pub fn attach(self: &Arc<Self>, tx: Sender<Msg>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.tx = Some(tx);
        if !state.thread_spawned {
            state.thread_spawned = true;
            let bg = Arc::clone(self);
            thread::spawn(move || bg.run());
        }
        drop(state);
        self.condvar.notify_one();
    }

    pub fn arm(&self, key: TimerKey, delay: Duration, msg: Msg) {
        let deadline = Instant::now() + delay;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending.insert(key, (deadline, msg));
        drop(state);
        self.condvar.notify_one();
    }

    fn run(self: Arc<Self>) {
        loop {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            let now = Instant::now();
            let due_keys: Vec<TimerKey> = state
                .pending
                .iter()
                .filter(|&(_, &(deadline, _))| deadline <= now)
                .map(|(&key, _)| key)
                .collect();
            let mut due = Vec::new();
            for key in due_keys {
                if let Some((_, msg)) = state.pending.remove(&key) {
                    due.push(msg);
                }
            }

            if !due.is_empty() {
                if let Some(tx) = state.tx.clone() {
                    drop(state);
                    for msg in due {
                        let _ = tx.send(msg);
                    }
                }
                continue;
            }

            let next_deadline = state.pending.values().map(|&(d, _)| d).min();
            match next_deadline {
                Some(deadline) => {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    let (_state, _timed_out) = self
                        .condvar
                        .wait_timeout(state, wait)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                None => {
                    let _state = self
                        .condvar
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;
    use std::sync::mpsc;
    use std::time::Duration;

    fn doc_id(n: u64) -> DocumentId {
        DocumentId(NonZeroU64::new(n).expect("nonzero"))
    }

    #[test]
    fn arming_without_attach_never_panics_or_blocks() {
        let timer = TimerService::new();
        timer.arm(
            TimerKey::Snapshot(doc_id(1)),
            Duration::ZERO,
            Msg::SnapshotDue {
                id: doc_id(1),
                generation: 1,
            },
        );
    }

    #[test]
    fn fires_exactly_once_after_its_deadline() {
        let timer = TimerService::new();
        let (tx, rx) = mpsc::channel();
        timer.attach(tx);
        timer.arm(
            TimerKey::Snapshot(doc_id(1)),
            Duration::from_millis(20),
            Msg::SnapshotDue {
                id: doc_id(1),
                generation: 7,
            },
        );

        let msg = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the timer must fire after its deadline");
        match msg {
            Msg::SnapshotDue { id, generation } => {
                assert_eq!(id, doc_id(1));
                assert_eq!(generation, 7);
            }
            other => panic!("expected Msg::SnapshotDue, got {other:?}"),
        }
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "a single arm must fire exactly once, not repeat"
        );
    }

    #[test]
    fn rearming_the_same_key_replaces_its_deadline() {
        let timer = TimerService::new();
        let (tx, rx) = mpsc::channel();
        timer.attach(tx);

        timer.arm(
            TimerKey::Snapshot(doc_id(1)),
            Duration::from_millis(500),
            Msg::SnapshotDue {
                id: doc_id(1),
                generation: 1,
            },
        );
        timer.arm(
            TimerKey::Snapshot(doc_id(1)),
            Duration::from_millis(20),
            Msg::SnapshotDue {
                id: doc_id(1),
                generation: 2,
            },
        );

        let msg = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the shorter, later arm must fire");
        match msg {
            Msg::SnapshotDue { id, generation } => {
                assert_eq!(id, doc_id(1));
                assert_eq!(generation, 2, "the re-arm's generation must win");
            }
            other => panic!("expected Msg::SnapshotDue, got {other:?}"),
        }
        assert!(
            rx.recv_timeout(Duration::from_millis(600)).is_err(),
            "the superseded first arm must never fire on its own"
        );
    }

    #[test]
    fn distinct_keys_fire_independently() {
        let timer = TimerService::new();
        let (tx, rx) = mpsc::channel();
        timer.attach(tx);

        timer.arm(
            TimerKey::Snapshot(doc_id(1)),
            Duration::from_millis(20),
            Msg::SnapshotDue {
                id: doc_id(1),
                generation: 1,
            },
        );
        timer.arm(
            TimerKey::QuitConfirm,
            Duration::from_millis(60),
            Msg::Timer {
                key: TimerKey::QuitConfirm,
                generation: 1,
            },
        );

        let mut seen_snapshot = false;
        let mut seen_quit = false;
        for _ in 0..2 {
            let msg = rx
                .recv_timeout(Duration::from_secs(2))
                .expect("both keys must fire");
            match msg {
                Msg::SnapshotDue { .. } => seen_snapshot = true,
                Msg::Timer { .. } => seen_quit = true,
                other => panic!("unexpected message: {other:?}"),
            }
        }
        assert!(seen_snapshot && seen_quit);
    }
}
