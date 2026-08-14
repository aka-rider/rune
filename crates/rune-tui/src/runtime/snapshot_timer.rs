//! One rearmable timer thread per app for the snapshot-autosave debounce
//! (plan WP16.S5), replacing the previous design's fresh `std::thread::
//! sleep` `Cmd` spawn on every message that mutated a document's journal —
//! a document typed into continuously re-armed a brand new OS thread on
//! every keystroke, each of which just slept 2s and then (almost always)
//! lost the generation race to the NEXT keystroke's own fresh thread. This
//! keeps exactly one background thread for the whole app: it parks on a
//! `Condvar` until the EARLIEST pending document's deadline, and `arm`
//! merely records/overwrites that document's own deadline and wakes it —
//! no new thread, no old thread to cancel.
//!
//! Mirrors `db::DbBridge`'s bootstrap/live split: [`SnapshotTimer::new`]
//! creates no thread at all (a test/fuzz `App` that never runs the real
//! runtime loop only ever calls [`SnapshotTimer::arm`], a pure state update
//! with nothing parked on it to wake) — the ONE background thread starts
//! at [`SnapshotTimer::attach`], called once from `runtime::run` exactly
//! like `DbBridge::attach`.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::document::DocumentId;

use super::Msg;

struct TimerState {
    /// `id`'s currently armed `(generation, deadline)` — inserting again
    /// for the same `id` overwrites the previous entry, so a document
    /// edited twice within the debounce window has exactly one pending
    /// deadline: the later one.
    pending: HashMap<DocumentId, (u32, Instant)>,
    tx: Option<Sender<Msg>>,
    thread_spawned: bool,
}

pub struct SnapshotTimer {
    state: Mutex<TimerState>,
    condvar: Condvar,
}

impl SnapshotTimer {
    pub fn new() -> Arc<SnapshotTimer> {
        Arc::new(SnapshotTimer {
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

    /// (Re)arms `id`'s debounce to fire `Msg::SnapshotDue { id, generation }`
    /// after `delay` — overwrites any earlier pending deadline for the same
    /// `id`. The deadline is computed HERE, from this timer thread's own
    /// `Instant::now()` — a timeout is a message produced by a dedicated
    /// thread, so it reads its own clock rather than accepting an absolute
    /// `Instant` from the caller's (possibly different, possibly manual in
    /// tests) time domain. A pure state update before `attach` has ever run
    /// (or in a test/fuzz `App` that never calls it): nothing is parked on
    /// the `Condvar` yet, so `notify_one` is simply a no-op.
    pub fn arm(&self, id: DocumentId, generation: u32, delay: Duration) {
        let deadline = Instant::now() + delay;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending.insert(id, (generation, deadline));
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
            let mut due = Vec::new();
            state.pending.retain(|&id, &mut (generation, deadline)| {
                if deadline <= now {
                    due.push((id, generation));
                    false
                } else {
                    true
                }
            });

            if !due.is_empty() {
                if let Some(tx) = state.tx.clone() {
                    drop(state);
                    // A closed `tx` means the main loop already exited —
                    // nothing left to schedule a snapshot for.
                    for (id, generation) in due {
                        let _ = tx.send(Msg::SnapshotDue { id, generation });
                    }
                }
                continue;
            }

            let next_deadline = state.pending.values().map(|&(_, d)| d).min();
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
    /// through `save::schedule_snapshot_debounce` but never runs the real
    /// runtime loop that would `attach` it.
    fn timer_without_attach() {
        let timer = SnapshotTimer::new();
        timer.arm(doc_id(1), 1, Duration::ZERO);
        // No assertion beyond "did not panic/hang" — there is nothing to
        // observe without a channel attached.
    }

    #[test]
    fn arming_without_attach_never_panics_or_blocks() {
        timer_without_attach();
    }

    #[test]
    fn fires_exactly_once_after_its_deadline() {
        let timer = SnapshotTimer::new();
        let (tx, rx) = mpsc::channel();
        timer.attach(tx);
        timer.arm(doc_id(1), 7, Duration::from_millis(20));

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

    /// Re-arming the SAME document before its deadline must replace it, not
    /// queue a second fire — the whole point of the debounce.
    #[test]
    fn rearming_the_same_document_replaces_its_deadline() {
        let timer = SnapshotTimer::new();
        let (tx, rx) = mpsc::channel();
        timer.attach(tx);

        timer.arm(doc_id(1), 1, Duration::from_millis(500));
        // Re-arm almost immediately with a LATER generation and a much
        // sooner deadline — the later arm must win outright.
        timer.arm(doc_id(1), 2, Duration::from_millis(20));

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

    /// Two different documents debounced concurrently each get their own
    /// independent fire — one document's deadline must never suppress or
    /// merge with another's.
    #[test]
    fn two_documents_debounce_independently() {
        let timer = SnapshotTimer::new();
        let (tx, rx) = mpsc::channel();
        timer.attach(tx);

        timer.arm(doc_id(1), 1, Duration::from_millis(20));
        timer.arm(doc_id(2), 1, Duration::from_millis(60));

        let mut seen = Vec::new();
        for _ in 0..2 {
            let msg = rx
                .recv_timeout(Duration::from_secs(2))
                .expect("both documents must fire");
            if let Msg::SnapshotDue { id, .. } = msg {
                seen.push(id);
            }
        }
        seen.sort();
        let mut expected = vec![doc_id(1), doc_id(2)];
        expected.sort();
        assert_eq!(seen, expected);
    }
}
