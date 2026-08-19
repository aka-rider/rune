//! One rearmable timer thread per app, shared by every debounce/timeout
//! that used to spawn its own fresh `std::thread::sleep` `Cmd` on every
//! (re)arm — the snapshot-autosave debounce, the degraded-save confirm
//! gate, the quit-confirm window, and the message pane's auto-collapse.
//! Each of those used to pay for a brand new OS thread per keystroke/press,
//! almost always losing the generation race to the NEXT press's own fresh
//! thread. This keeps exactly one background thread for the whole app: it
//! parks on a `Condvar` until the EARLIEST pending key's deadline, and
//! `arm` merely records/overwrites that key's own deadline (and the exact
//! `Msg` to fire once it elapses) and wakes it — no new thread, no old
//! thread to cancel.
//!
//! Mirrors `db::DbBridge`'s bootstrap/live split: [`TimerService::new`]
//! creates no thread at all (a test/fuzz `App` that never runs the real
//! runtime loop only ever calls [`TimerService::arm`], a pure state update
//! with nothing parked on it to wake) — the ONE background thread starts
//! at [`TimerService::attach`], called once from `runtime::run` exactly
//! like `DbBridge::attach`.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::document::DocumentId;

use super::Msg;

/// Which debounce/timeout a pending deadline belongs to — the map key of
/// [`TimerService`]'s one pending-deadline table. `Snapshot` is keyed per
/// document (many documents can debounce independently); the other three
/// are each a single app-wide slot, matching the single global `App` field
/// each one's own arm site already gates through (`pending_save_confirm`,
/// `QuitNegotiation`, `MessageLog::armed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimerKey {
    Snapshot(DocumentId),
    SaveConfirm,
    QuitConfirm,
    MessagesCollapse,
}

struct TimerState {
    /// `key`'s currently armed `(deadline, Msg)` — inserting again for the
    /// same `key` overwrites the previous entry, so a key armed twice
    /// within its own window has exactly one pending deadline: the later
    /// one. The `Msg` to fire is captured at arm time (not recomputed at
    /// fire time), so this map stays generic over every consumer's own
    /// `Msg` shape and generation type without the timer needing to know
    /// either.
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

    /// Wires this timer to the runtime's real `Msg` channel and starts its
    /// one background thread — idempotent (a second call only updates
    /// `tx`, never spawns a second thread). Called exactly once, from
    /// `runtime::run`, mirroring `DbBridge::attach`.
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

    /// (Re)arms `key`'s deadline to fire `msg` after `delay` — overwrites
    /// any earlier pending deadline (and `Msg`) for the same `key`. The
    /// deadline is computed HERE, from this timer thread's own
    /// `Instant::now()` — a timeout is a message produced by a dedicated
    /// thread, so it reads its own clock rather than accepting an absolute
    /// `Instant` from the caller's (possibly different, possibly manual in
    /// tests) time domain. A pure state update before `attach` has ever run
    /// (or in a test/fuzz `App` that never calls it): nothing is parked on
    /// the `Condvar` yet, so `notify_one` is simply a no-op.
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

    /// The one background thread's whole loop: fire every deadline that has
    /// already passed, then park until the next-earliest one (or
    /// indefinitely, woken only by `arm`/`attach`, if nothing is pending).
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
                    // A closed `tx` means the main loop already exited —
                    // nothing left to schedule anything for.
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

    /// Before `attach`, `arm` must be a pure state update — nothing panics,
    /// nothing blocks, and (since there is no thread yet) nothing ever
    /// fires. Matches how a test/fuzz `App` uses this type: it calls `arm`
    /// through e.g. `save::schedule_snapshot_debounce` but never runs the
    /// real runtime loop that would `attach` it.
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

    /// Re-arming the SAME key before its deadline must replace it, not
    /// queue a second fire — the whole point of the debounce.
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
        // Re-arm almost immediately with a LATER generation and a much
        // sooner deadline — the later arm must win outright.
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

    /// Two different keys debounced concurrently each get their own
    /// independent fire — one key's deadline must never suppress or merge
    /// with another's, whether they're two documents' snapshots or two
    /// distinct timeout kinds entirely.
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
