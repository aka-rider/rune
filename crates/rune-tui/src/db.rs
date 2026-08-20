//! Wiring between `rune-tui`'s Elm-style runtime and `rune-db`'s async
//! writer-thread `Store`: the `DbEvent` -> `Msg::Db` bridge, the
//! app-level `Db` handle (the `Store` itself + the bridge + the sticky
//! degraded flag), and the per-document `DocDb` handle (this doc's bound
//! row plus its async-replica bookkeeping). The
//! in-memory `rune_core::undo::Journal` stays the synchronous, authoritative
//! source of truth for the running session — nothing here ever waits on a
//! `Store` ack before mutating the buffer, and every call
//! is a plain, non-blocking channel send (`Store::enqueue`'s `try_send`),
//! never I/O — so these are called directly from `update`, not from a
//! spawned `Cmd`.
//!
//! One `Store` is shared by every open document (`App::db: Option<Db>`);
//! each document binds its own row via `DocDb::db_id` (formerly `doc_id`).
//! Because the writer thread processes one ordered FIFO across ALL
//! documents, a `DbEvent` ack's `id` alone doesn't say which document it
//! belongs to — `App::db_ops: HashMap<u64, PendingOp>` records that mapping,
//! plus (for a `Load` op) the buffer version it was issued against, at every
//! successful enqueue and is consulted/popped by
//! `app::handle_db_event`.
//!
//! The functions building and submitting ops into `db_ops`
//! (`commands::edit::commit_edit_batch`/`undo`/`redo`'s call sites) live in
//! [`crate::db_enqueue`]; the reaction to their eventual acks lives in
//! [`crate::db_ack`]. This module owns only the shared types both sides
//! need.

use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};

use rune_db::{DbEvent, ObsId, OnEvent, Store};

use crate::document::DocumentId;
use crate::runtime::Msg;

/// Where a `DbEvent` goes before vs after the runtime loop exists. See
/// [`DbBridge`]'s doc comment for why this indirection is necessary at all.
/// `Bootstrap` buffers every event it's handed, in arrival order — it is
/// never a channel to something that might already have stopped listening,
/// so a `DbEvent` delivered in this state cannot be lost, only queued.
enum Sink {
    Bootstrap(VecDeque<DbEvent>),
    Live(Sender<Msg>),
}

/// Adapts every `DbEvent` the `rune-db` writer thread posts into this
/// crate's `Msg` channel. Constructed once at bootstrap (`rune-cli::main`),
/// BEFORE `runtime::run` creates its own `Sender<Msg>` — the
/// runtime never exposes its `Sender<Msg>`; `runtime.rs` creates it
/// privately — `Store::open`/`open_in_memory` (also bootstrap-time, so
/// hydration can finish before the TUI ever draws a frame) take
/// their `on_event` callback fixed at construction, with no way to swap it
/// afterward.
///
/// Between that construction and [`DbBridge::attach`] there is a whole
/// window — hydrating the first file, then opening every extra CLI
/// positional through `workspace::open_path` — during which the writer
/// thread may post acks for MORE than just the one op bootstrap hydration
/// is synchronously waiting on (every extra file's own `Load`). Those acks
/// have nowhere else to go yet: `runtime::run` hasn't built its `Sender
/// <Msg>` and may not for a while (opening N tabs, then constructing the
/// terminal). A design routing them through an external channel whose
/// receiving end could already be gone (bootstrap hydration's own blocking
/// wait finishes and drops it well before those tabs are even opened) is
/// exactly how they used to go missing. Instead, `Bootstrap` buffers
/// directly on `self` — nothing outside this type has to stay alive for a
/// `DbEvent` to survive the handover, and `attach` drains that buffer into
/// the live `Msg` channel before switching over, so nothing posted during
/// the window is ever lost.
pub struct DbBridge {
    sink: Mutex<Sink>,
    /// Wakes [`DbBridge::wait_for_bootstrap_event`] whenever `deliver`
    /// pushes onto a still-`Bootstrap` sink.
    arrived: Condvar,
}

impl DbBridge {
    /// Constructs a bridge in its `Bootstrap` state.
    pub fn bootstrap() -> Arc<DbBridge> {
        Arc::new(DbBridge {
            sink: Mutex::new(Sink::Bootstrap(VecDeque::new())),
            arrived: Condvar::new(),
        })
    }

    /// The `Store::open`/`open_in_memory` `on_event` callback.
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
                // The runtime loop's `Receiver<Msg>` outlives every `Db`
                // this bridge can still receive events for — it drops only
                // after `Store::shutdown` has drained the writer thread
                // (`rune-cli::main`'s exit sequence) — so a send failure
                // here means there is no loop left to act on the event
                // either way.
                let _ = tx.send(Msg::Db(evt));
            }
        }
    }

    /// Blocks the CALLING thread — bootstrap hydration, before any runtime
    /// loop or `Msg` channel exists — until an event matching `pred`
    /// arrives, then removes and returns exactly that one. Any OTHER event
    /// delivered in the meantime (an extra file's own `Load` ack racing
    /// ahead of the one hydration is waiting on) stays in the buffer for
    /// [`DbBridge::attach`] to drain later, so this synchronous wait can
    /// never consume or discard a sibling document's event.
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

    /// Switches the bridge to `Live`: every subsequent `DbEvent` is wrapped
    /// as `Msg::Db(...)` and delivered through `tx` instead. Drains
    /// whatever accumulated in the `Bootstrap` buffer first, in arrival
    /// order, so an ack that arrived before this call is still delivered
    /// rather than left stranded once the sink switches over.
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

/// Why a `Load` op was issued. `Recover` adopts whatever the store
/// recovered into the buffer; `Rebaseline` only refreshes an already-live
/// document's CAS baseline and must never hydrate, or the user's own typing
/// is replaced by a stale row's content the instant the round trip lands.
/// `expect_row` is the row the document was bound to when the op was
/// ENQUEUED — the same value the writer thread was given, so both sides
/// decide "same row" from one fact rather than from two samplings. `None`
/// means there was no binding to preserve.
#[derive(Clone, Copy)]
pub enum LoadPurpose {
    Recover,
    Rebaseline { expect_row: Option<i64> },
}

/// The document a recovery-store op belongs to, and — for a `Load` op only —
/// the buffer version it was issued against. These two facts are always
/// inserted and removed together for the same op id; carrying them in one
/// value (rather than two maps keyed by the same id) makes it impossible for
/// a sweep to drop one fact while leaving the other behind.
pub struct PendingOp {
    pub doc: DocumentId,
    /// The issuing document's `buffer.version()` at the moment a `Load` op
    /// was enqueued — `None` for every other op kind, which never needs it.
    pub issued_version: Option<u64>,
    /// True iff this op is a `Probe` — lets
    /// `workspace::switch_to` skip enqueueing a second probe for a document
    /// that already has one in flight (`probe.rs`'s own doc comment: each
    /// probe reads the whole file and inserts a fresh observation, so
    /// stacking redundant ones on every rapid tab switch would grow the
    /// store unboundedly for no new information).
    pub is_probe: bool,
    /// `Some(baseline_epoch)` iff this op's ack carries a verdict computed
    /// against the file's CAS baseline (a `Probe` or a
    /// `MaterializePrepare`) — the issuing document's
    /// `FileBinding::baseline_epoch` at enqueue time. The ack handler
    /// compares this against `FileBinding::baseline_epoch` as it stands
    /// when the reply lands: a baseline rewrite in between (a materialize
    /// publish, a merge's terminal adoption, an abandon's retraction) means
    /// the verdict this op computed no longer describes the current world,
    /// and the reply is dropped rather than trusted — the same
    /// generation-echo shape `merge_gen` below uses, scoped to one file's
    /// own baseline lineage instead of a single app-wide counter.
    pub baseline_epoch: Option<u32>,
    /// `Some(generation)` iff this op is a `MergePrep` — the
    /// generation `merge::begin` minted for this attempt at enqueue time.
    /// The landing handler (`merge::handle_merge_prep_ack`) compares this
    /// against `App.merge`'s OWN current `Pending` generation for the same
    /// document before trusting the ack: a later `^M` superseding this
    /// attempt before it lands must not have this stale ack mistaken for
    /// the current one (mirrors `is_probe`'s in-flight bookkeeping, but a
    /// generation counter rather than a plain flag, since more than one
    /// merge attempt can be in flight in sequence for the same document).
    pub merge_gen: Option<crate::generation::MergeGen>,
    /// Why this `Load` was issued — [`LoadPurpose`]. Every other op kind
    /// records [`LoadPurpose::Recover`] and never reads it back.
    pub load_purpose: LoadPurpose,
    /// True iff this op only READS one document's disk/journal state
    /// (`Probe`/`Load`/`MergePrep`): its failure means "this document's
    /// read didn't land", never "the store can't be trusted for recovery",
    /// so the `DbEvent::Err` router posts a per-document error instead of
    /// sticky-degrading the whole store.
    pub doc_scoped: bool,
    /// True iff this op is an `AppendEdit` — lets `db_ack::
    /// bind_document_row` count the appends still in flight past a
    /// same-row re-baseline `Load`, which are exactly the entries the
    /// writer's restarted numbering already holds.
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

    /// An `AppendEdit` op — [`PendingOp::new`] with the append marker
    /// `db_ack::bind_document_row` counts (`is_append`'s own doc comment).
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

    /// A `MoveUndoPos` op — doc-scoped: resolving a local undo position to
    /// a durable seq can fail on this ONE document's own local-position
    /// bookkeeping (`rune_db::OpKind::MoveUndoPos`'s doc comment) without
    /// that being any kind of evidence the store itself can no longer be
    /// trusted for recovery — an undo error must never sticky-degrade the
    /// whole store.
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

/// The app-wide half of the old `AppDb`: the `Store`
/// itself, the bridge routing its acks into `Msg::Db`, and the sticky
/// degraded flag — shared by every open `Document`. Per-document state
/// (bound row, CAS baseline, async-replica bookkeeping) lives on `DocDb`
/// below, one per `Document`.
pub struct Db {
    pub store: Store,
    pub bridge: Arc<DbBridge>,
    /// True once this store can no longer be trusted for recovery — either
    /// the open ladder degraded to `:memory:` at launch, or a LATER
    /// enqueue-time error / `DbEvent::Err`/`Fatal`: on
    /// hard write failure the buffer is never rolled back; the store
    /// enters degraded mode. Sticky for the process lifetime — there is no
    /// reopen/reconnect path.
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

    /// Deterministically drains and joins the underlying `Store`'s
    /// writer/reader threads (`Store::shutdown`'s own doc comment) — the
    /// one place `rune-cli::main` closes the recovery store on every exit
    /// path, not just its own bootstrap-failure branches.
    pub fn shutdown(self) {
        self.store.shutdown();
    }
}

/// This document's handle onto the app-wide recovery store, split out of
/// the old `AppDb`: the bound `documents` row
/// id (`db_id`, formerly `doc_id` — renamed so it can't be confused with
/// `DocumentId`, the in-process tab identity) and the async-replica
/// bookkeeping reconciling the LOCAL journal against the DURABLE one. The
/// CAS baseline itself — `expect_obs`/`pending_rebaseline_hash`/`baseline_epoch`
/// — lives on [`FileBinding`], shared by every `Document` bound to the same
/// `db_id`, never copied here: two tabs opened onto the SAME underlying file
/// must see the one truth about what disk holds, or one tab's own save
/// falsely raises the disk-conflict guard against the other's very next
/// attempt.
/// How this document's NEXT save publishes: [`PublishMode::CreateOnly`]
/// until the first successful create commits (no CAS baseline exists yet,
/// so the publish is an atomic no-clobber `rename_excl` that must not
/// overwrite a concurrent creator's file), [`PublishMode::OverwriteExisting`]
/// once an established baseline exists (an ordinary compare-and-swap
/// overwrite). [`PublishMode::materialize_target`] is the ONE conversion
/// onto `rune_db::MaterializeTarget`.
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
