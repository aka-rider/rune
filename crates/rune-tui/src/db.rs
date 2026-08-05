//! Wiring between `rune-tui`'s Elm-style runtime and `rune-db`'s async
//! writer-thread `Store` (plan WP5, re-split in WP1 for multi-document
//! support — plan WP1 decision 5): the `DbEvent` -> `Msg::Db` bridge, the
//! app-level `Db` handle (the `Store` itself + the bridge + the sticky
//! degraded flag), and the per-document `DocDb` handle (this doc's bound
//! row plus its async-replica bookkeeping). CONSTITUTION §1.4.8/§5.4: the
//! in-memory `rune_core::undo::Journal` stays the synchronous, authoritative
//! source of truth for the running session — nothing here ever waits on a
//! `Store` ack before mutating the buffer (plan decision 3), and every call
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
//! successful enqueue (plan decision 6) and is consulted/popped by
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
/// BEFORE `runtime::run` creates its own `Sender<Msg>` (plan Gotchas: "the
/// runtime never exposes its `Sender<Msg>`" — `runtime.rs` creates it
/// privately) — `Store::open`/`open_in_memory` (also bootstrap-time, so
/// hydration, WP5.S4, can finish before the TUI ever draws a frame) take
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
        let mut sink = self.sink.lock().unwrap_or_else(|p| p.into_inner());
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
        let mut sink = self.sink.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Sink::Bootstrap(buf) = &mut *sink
                && let Some(pos) = buf.iter().position(&mut pred)
                && let Some(evt) = buf.remove(pos)
            {
                return evt;
            }
            sink = self.arrived.wait(sink).unwrap_or_else(|p| p.into_inner());
        }
    }

    /// Switches the bridge to `Live`: every subsequent `DbEvent` is wrapped
    /// as `Msg::Db(...)` and delivered through `tx` instead. Drains
    /// whatever accumulated in the `Bootstrap` buffer first, in arrival
    /// order, so an ack that arrived before this call is still delivered
    /// rather than left stranded once the sink switches over.
    pub fn attach(&self, tx: Sender<Msg>) {
        let mut sink = self.sink.lock().unwrap_or_else(|p| p.into_inner());
        if let Sink::Bootstrap(buf) = &mut *sink {
            for evt in buf.drain(..) {
                let _ = tx.send(Msg::Db(evt));
            }
        }
        *sink = Sink::Live(tx);
    }
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
    /// True iff this op is a `CreateScratch` minting `doc`'s OWN recovery
    /// row (`db_enqueue::create_scratch`). `CreateSnapshot` also resolves to
    /// `OpOutcome::RowId` (`materialize_ack::handle_snapshot_due`'s own
    /// enqueue), so the ack router needs this flag to tell "bind a fresh
    /// `DocDb`" apart from "a snapshot anchor landed, nothing else to do" —
    /// both share the same outcome shape but need opposite reactions.
    pub mints_scratch: bool,
    /// True iff this op is a `Probe` (plan WP2.S4) — lets
    /// `workspace::switch_to` skip enqueueing a second probe for a document
    /// that already has one in flight (`probe.rs`'s own doc comment: each
    /// probe reads the whole file and inserts a fresh observation, so
    /// stacking redundant ones on every rapid tab switch would grow the
    /// store unboundedly for no new information).
    pub is_probe: bool,
}

impl PendingOp {
    pub fn new(doc: DocumentId) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            mints_scratch: false,
            is_probe: false,
        }
    }

    pub fn load(doc: DocumentId, issued_version: u64) -> PendingOp {
        PendingOp {
            doc,
            issued_version: Some(issued_version),
            mints_scratch: false,
            is_probe: false,
        }
    }

    pub fn create_scratch(doc: DocumentId) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            mints_scratch: true,
            is_probe: false,
        }
    }

    pub fn probe(doc: DocumentId) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            mints_scratch: false,
            is_probe: true,
        }
    }
}

/// The app-wide half of the old `AppDb` (plan WP1 decision 5): the `Store`
/// itself, the bridge routing its acks into `Msg::Db`, and the sticky
/// degraded flag — shared by every open `Document`. Per-document state
/// (bound row, CAS baseline, async-replica bookkeeping) lives on `DocDb`
/// below, one per `Document`.
pub struct Db {
    pub store: Store,
    pub bridge: Arc<DbBridge>,
    /// True once this store can no longer be trusted for recovery — either
    /// the open ladder degraded to `:memory:` at launch, or a LATER
    /// enqueue-time error / `DbEvent::Err`/`Fatal` (plan decision 3: "on
    /// hard write failure the buffer is never rolled back; the store
    /// enters degraded mode"). Sticky for the process lifetime — WP1 has no
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

/// This document's handle onto the app-wide recovery store (plan WP1
/// decision 5, split out of the pre-WP1 `AppDb`): the bound `documents` row
/// id (`db_id`, formerly `doc_id` — renamed so it can't be confused with
/// `DocumentId`, the in-process tab identity), this session's current CAS
/// baseline, and the async-replica bookkeeping reconciling the LOCAL
/// journal against the DURABLE one.
pub struct DocDb {
    pub db_id: i64,
    /// This session's current CAS baseline for `db_id` — updated from
    /// every successful `materialize` ack's `saved` observation (plan
    /// WP5.S6). Seeded from `LoadResult::saved_obs` at hydration.
    pub expect_obs: ObsId,
    /// Whether the NEXT save must go through `materialize`'s `bind_new`
    /// (create-only, `rename_excl`) path rather than the CAS-overwrite path
    /// — true until the first successful create commits.
    pub bind_new: bool,
    /// The highest durable journal seq (`events.seq`) this session has SEEN
    /// acknowledged so far for this document — a conservative stand-in for
    /// "the durable journal's current head", used as `materialize`'s `seq`
    /// parameter and as the fallback when `seq_by_local_pos` doesn't have an
    /// exact answer yet (see its doc comment).
    pub last_known_seq: i64,
    /// `seq_by_local_pos[i]` is the durable seq that local `Journal`
    /// position `i + 1` (i.e., after the `(i+1)`-th local `push`)
    /// committed to, once its `AppendEdit` ack lands — `None` until then.
    /// Reconciles the LOCAL in-memory journal's plain step-COUNT position
    /// against the DURABLE journal's `events.seq` numbering, which can fall
    /// behind it when the durable side coalesces two local pushes into the
    /// SAME row (`journal.rs`'s coalescing guards) — read by
    /// `commands::edit`'s `undo`/`redo` when committing `move_undo_pos`.
    pub seq_by_local_pos: Vec<Option<i64>>,
    /// FIFO of local positions still awaiting their `AppendEdit` ack,
    /// oldest first — the writer thread's single ordered queue guarantees
    /// THIS document's `AppendEdit` acks land in the same relative order
    /// its own ops were enqueued (a subsequence of the global FIFO order),
    /// so the oldest entry here is always the next ack to fill in.
    pub pending_seq_acks: VecDeque<usize>,
    /// Bumped on every journal mutation; the debounce token for the 2s
    /// snapshot-autosave timer (plan WP5.S6, port of
    /// `workspace_timers.go`) — a `Msg::SnapshotDue` arriving with a
    /// stale generation means a later edit already superseded it, so it's
    /// ignored.
    pub snapshot_generation: u32,
}

impl DocDb {
    pub fn new(db_id: i64, expect_obs: ObsId, bind_new: bool, last_known_seq: i64) -> DocDb {
        DocDb {
            db_id,
            expect_obs,
            bind_new,
            last_known_seq,
            seq_by_local_pos: Vec::new(),
            pending_seq_acks: VecDeque::new(),
            snapshot_generation: 0,
        }
    }

    /// Records that local `Journal` position `local_pos` (`Journal::pos()`
    /// AFTER the push it corresponds to) has been enqueued as an
    /// `AppendEdit` — reserves its slot in `seq_by_local_pos` so a LATER ack
    /// (which may arrive after several more local pushes) fills in the
    /// right index.
    pub(crate) fn note_pending_append(&mut self, local_pos: usize) {
        if self.seq_by_local_pos.len() < local_pos {
            self.seq_by_local_pos.resize(local_pos, None);
        }
        self.pending_seq_acks.push_back(local_pos - 1);
    }

    /// Consumes the oldest pending `AppendEdit` ack, filling in its durable
    /// seq. A `DbEvent::Ok` `AppendEdit` ack with no matching pending entry
    /// (shouldn't happen — every enqueue notes one first) is ignored rather
    /// than indexing out of bounds.
    pub(crate) fn resolve_append_ack(&mut self, seq: i64) {
        self.last_known_seq = self.last_known_seq.max(seq);
        if let Some(idx) = self.pending_seq_acks.pop_front()
            && let Some(slot) = self.seq_by_local_pos.get_mut(idx)
        {
            *slot = Some(seq);
        }
    }

    /// The durable seq local `Journal` position `local_pos` corresponds to
    /// — `0` for "nothing this session has committed" (matches `rune-db`'s
    /// own `current_seq` "no row" default), else the exact acked seq if
    /// known, else `last_known_seq` as a conservative approximation for a
    /// still-in-flight ack (acks arrive in enqueue order, so this never
    /// OVERestimates in practice).
    pub(crate) fn seq_for_local_pos(&self, local_pos: usize) -> i64 {
        if local_pos == 0 {
            return 0;
        }
        self.seq_by_local_pos
            .get(local_pos - 1)
            .copied()
            .flatten()
            .unwrap_or(self.last_known_seq)
    }
}
