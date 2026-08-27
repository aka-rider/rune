use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};

use rune_db::{DbEvent, ObsId, OnEvent, Store};

use crate::document::DocumentId;
use crate::runtime::Msg;

// `Store::open`'s `on_event` callback is fixed at construction, before the
// runtime has a live `Sender<Msg>` to route through, so events arriving in
// between accumulate here instead of being dropped.
enum Sink {
    Bootstrap(VecDeque<DbEvent>),
    Live(Sender<Msg>),
}

pub struct DbBridge {
    sink: Mutex<Sink>,
    arrived: Condvar,
}

impl DbBridge {
    pub fn bootstrap() -> Arc<DbBridge> {
        Arc::new(DbBridge {
            sink: Mutex::new(Sink::Bootstrap(VecDeque::new())),
            arrived: Condvar::new(),
        })
    }

    pub fn on_event(self: &Arc<Self>) -> OnEvent {
        let bridge = Arc::clone(self);
        Box::new(move |evt| bridge.deliver(evt))
    }

    fn deliver(&self, evt: DbEvent) {
        let mut sink = self
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *sink {
            Sink::Bootstrap(buf) => {
                buf.push_back(evt);
                self.arrived.notify_all();
            }
            Sink::Live(tx) => {
                // The receiver only drops once the writer thread has
                // already been drained on shutdown, so a send failure here
                // means there is no loop left to act on the event either way.
                let _ = tx.send(Msg::Db(evt));
            }
        }
    }

    pub fn wait_for_bootstrap_event(&self, mut pred: impl FnMut(&DbEvent) -> bool) -> DbEvent {
        let mut sink = self
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Sink::Bootstrap(buf) = &mut *sink
                && let Some(pos) = buf.iter().position(&mut pred)
                && let Some(evt) = buf.remove(pos)
            {
                return evt;
            }
            sink = self
                .arrived
                .wait(sink)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub fn attach(&self, tx: Sender<Msg>) {
        let mut sink = self
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Sink::Bootstrap(buf) = &mut *sink {
            for evt in buf.drain(..) {
                let _ = tx.send(Msg::Db(evt));
            }
        }
        *sink = Sink::Live(tx);
    }
}

// `Rebaseline` must never hydrate: replacing the buffer's live content with
// a stale row the instant the round trip lands would overwrite the user's
// own typing. `expect_row` pins the row this op was enqueued against, so
// both sides judge "same row" from one fact instead of two samplings.
#[derive(Clone, Copy)]
pub enum LoadPurpose {
    Recover,
    Rebaseline { expect_row: Option<i64> },
}

pub struct PendingOp {
    pub doc: DocumentId,
    pub issued_version: Option<u64>,
    // Each probe reads the whole file and inserts a fresh observation, so a
    // document already probing skips a redundant second one rather than
    // growing the store unboundedly for no new information.
    pub is_probe: bool,
    // Set only for a `Probe`/`MaterializePrepare` ack: if the CAS baseline
    // has moved on by the time the reply lands, the verdict no longer
    // describes the current world and the reply is dropped rather than
    // trusted.
    pub baseline_epoch: Option<u32>,
    // Set only for a `MergePrep`: the ack is trusted only if this still
    // matches the document's current pending attempt, so a later attempt
    // superseding this one can't have a stale ack mistaken for the live one.
    pub merge_gen: Option<crate::generation::MergeGen>,
    pub load_purpose: LoadPurpose,
    // True for an op that only reads one document's disk/journal state:
    // its failure means this document's read didn't land, never that the
    // store itself can no longer be trusted for recovery.
    pub doc_scoped: bool,
    // True for an `AppendEdit`: counts the appends still in flight past a
    // same-row rebaseline `Load`, which are exactly the entries the
    // writer's restarted numbering already holds.
    pub is_append: bool,
}

impl PendingOp {
    pub fn new(doc: DocumentId) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            is_probe: false,
            baseline_epoch: None,
            merge_gen: None,
            load_purpose: LoadPurpose::Recover,
            doc_scoped: false,
            is_append: false,
        }
    }

    pub fn append(doc: DocumentId) -> PendingOp {
        PendingOp {
            is_append: true,
            ..PendingOp::new(doc)
        }
    }

    pub fn load(doc: DocumentId, issued_version: u64, load_purpose: LoadPurpose) -> PendingOp {
        PendingOp {
            doc,
            issued_version: Some(issued_version),
            is_probe: false,
            baseline_epoch: None,
            merge_gen: None,
            load_purpose,
            doc_scoped: true,
            is_append: false,
        }
    }

    pub fn probe(doc: DocumentId, baseline_epoch: u32) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            is_probe: true,
            baseline_epoch: Some(baseline_epoch),
            merge_gen: None,
            load_purpose: LoadPurpose::Recover,
            doc_scoped: true,
            is_append: false,
        }
    }

    pub fn prepare(doc: DocumentId, baseline_epoch: u32) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            is_probe: false,
            baseline_epoch: Some(baseline_epoch),
            merge_gen: None,
            load_purpose: LoadPurpose::Recover,
            doc_scoped: false,
            is_append: false,
        }
    }

    pub fn move_undo_pos(doc: DocumentId) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            is_probe: false,
            baseline_epoch: None,
            merge_gen: None,
            load_purpose: LoadPurpose::Recover,
            doc_scoped: true,
            is_append: false,
        }
    }

    pub fn merge_prep(doc: DocumentId, generation: crate::generation::MergeGen) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            is_probe: false,
            baseline_epoch: None,
            merge_gen: Some(generation),
            load_purpose: LoadPurpose::Recover,
            doc_scoped: true,
            is_append: false,
        }
    }
}

pub struct Db {
    pub store: Store,
    pub bridge: Arc<DbBridge>,
    // Sticky for the process lifetime once set: there is no
    // reopen/reconnect path once this store can no longer be trusted for
    // recovery.
    pub degraded: bool,
}

impl Db {
    pub fn new(store: Store, bridge: Arc<DbBridge>, degraded: bool) -> Db {
        Db {
            store,
            bridge,
            degraded,
        }
    }

    pub fn shutdown(self) {
        self.store.shutdown();
    }
}

// CreateOnly until the first successful create commits (no CAS baseline
// exists yet, so the publish is an atomic no-clobber `rename_excl` that
// must not overwrite a concurrent creator's file); OverwriteExisting once
// an established baseline exists (an ordinary compare-and-swap overwrite).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PublishMode {
    CreateOnly,
    OverwriteExisting,
}

impl PublishMode {
    pub fn is_create_only(self) -> bool {
        matches!(self, PublishMode::CreateOnly)
    }

    pub(crate) fn materialize_target(
        self,
        expect_obs: Option<rune_db::ObsId>,
    ) -> Option<rune_db::MaterializeTarget> {
        match self {
            PublishMode::CreateOnly => Some(rune_db::MaterializeTarget::BindNew),
            PublishMode::OverwriteExisting => {
                expect_obs.map(|expect| rune_db::MaterializeTarget::Existing { expect })
            }
        }
    }
}

pub use crate::db_types::{DocDb, FileBinding};

#[path = "db_app.rs"]
mod db_app;
