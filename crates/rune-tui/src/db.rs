//! Wiring between `rune-tui`'s Elm-style runtime and `rune-db`'s async
//! writer-thread `Store` (plan WP5, re-split in WP1 for multi-document
//! support — plan WP1 decision 5): the `DbEvent` -> `Msg::Db` bridge, the
//! app-level `Db` handle (the `Store` itself + the bridge + the sticky
//! degraded flag), the per-document `DocDb` handle (this doc's bound row +
//! its async-replica bookkeeping), and the small functions the three
//! journal call sites (`commands::edit::commit_edit_batch`/`undo`/`redo`)
//! need to talk to them. CONSTITUTION §1.4.8/§5.4: the in-memory
//! `rune_core::undo::Journal` stays the synchronous, authoritative source
//! of truth for the running session — nothing here ever waits on a `Store`
//! ack before mutating the buffer (plan decision 3), and every call below
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

use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;
use rune_db::{DbEvent, LoadResult, ObsId, OnEvent, Store};

use crate::app::{App, StatusSource};
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
}

impl PendingOp {
    pub fn new(doc: DocumentId) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
        }
    }

    pub fn load(doc: DocumentId, issued_version: u64) -> PendingOp {
        PendingOp {
            doc,
            issued_version: Some(issued_version),
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
    fn note_pending_append(&mut self, local_pos: usize) {
        if self.seq_by_local_pos.len() < local_pos {
            self.seq_by_local_pos.resize(local_pos, None);
        }
        self.pending_seq_acks.push_back(local_pos - 1);
    }

    /// Consumes the oldest pending `AppendEdit` ack, filling in its durable
    /// seq. A `DbEvent::Ok` `AppendEdit` ack with no matching pending entry
    /// (shouldn't happen — every enqueue notes one first) is ignored rather
    /// than indexing out of bounds.
    fn resolve_append_ack(&mut self, seq: i64) {
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
    fn seq_for_local_pos(&self, local_pos: usize) -> i64 {
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

/// Enqueues an `AppendEdit` replica of a batch this session just committed
/// to `id`'s LOCAL in-memory journal (plan WP5.S3) — called immediately
/// after `Journal::push` at `commands::edit::commit_edit_batch`'s one call
/// site. `local_pos` is `doc.journal.pos()` AFTER that push. A failure here
/// (enqueue-time `Error`, never an async one — that lands via `Msg::Db`
/// instead) only ever marks the whole store degraded
/// (`app::on_store_failure`) — the buffer/journal mutation already
/// happened and is never rolled back (plan decision 3). Every successful
/// enqueue records `id` in `app.db_ops` (plan decision 6) so the eventual
/// ack routes back to the right document.
pub fn append_edit(
    app: &mut App,
    id: DocumentId,
    local_pos: usize,
    edits: &[AppliedEdit],
    cursors_before: &[Cursor],
    cursors_after: &[Cursor],
) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    // `id` not (or no longer) live is a plain, correct no-op — see
    // `App::doc`'s docs.
    let Some(doc) = app.doc(id) else { return };
    let Some(db_id) = doc.db.as_ref().map(|d| d.db_id) else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };
    let result = db
        .store
        .append_edit(db_id, edits, cursors_before, cursors_after);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(id));
            if let Some(doc_db) = app.doc_mut(id).and_then(|d| d.db.as_mut()) {
                doc_db.note_pending_append(local_pos);
            }
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

/// Enqueues a `MoveUndoPos` replica of an undo/redo `id` just committed
/// locally (plan WP5.S3) — called immediately after `Journal::move_pos` at
/// `commands::edit::undo`/`redo`'s call sites. `local_pos` is the journal
/// position just committed (`Journal::move_pos`'s own argument).
pub fn move_undo_pos(app: &mut App, id: DocumentId, local_pos: usize) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let Some((target_seq, db_id)) = doc
        .db
        .as_ref()
        .map(|d| (d.seq_for_local_pos(local_pos), d.db_id))
    else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };
    let result = db.store.move_undo_pos(db_id, target_seq);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(id));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

/// Enqueues a `Load` op hydrating `id` (already bound to `path`, an
/// existing file just read straight off disk — `workspace::open_path`'s one
/// call site) through the app-wide recovery store, closing the "Explorer-
/// opened documents get no recovery journal" gap (plan WP6). Records `id`'s
/// buffer version at the moment the load is ISSUED, alongside the routing
/// entry, in one `PendingOp` in `app.db_ops` — `app::handle_db_event`'s
/// `Load` arm needs both to decide, on the ack, whether adopting the
/// recovered content is still safe (see `handle_load_ack`'s docs). A
/// degraded store enqueues nothing — there is no trustworthy recovery
/// journal to bind this document to either way.
pub fn load_document(app: &mut App, id: DocumentId, path: &Path) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let issued_version = doc.buffer.version();
    let Some(db) = app.db.as_ref() else { return };
    match db.store.load(path) {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::load(id, issued_version));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

/// The reaction to a `Load` op's ack (plan WP6.S2/S3) — routed from
/// `app::handle_db_event` once `app.db_ops` has resolved the ack's op id to
/// `id`. `issued_version` is `id`'s buffer version recorded by
/// `load_document` at ENQUEUE time, on the same `PendingOp` that resolved
/// `id`, `None` only if this ack's routing entry was somehow already
/// consumed.
///
/// A `None` `saved_obs` (should not occur — see `LoadResult::saved_obs`'s
/// own doc comment) installs nothing and surfaces a status message instead
/// of binding a document to a recovery row with no CAS baseline.
///
/// Otherwise, `recovered` is adopted into the buffer, through
/// [`crate::document::Document::hydrate`], ONLY when `issued_version` still
/// equals the buffer's CURRENT version — `Load` is asynchronous, so the
/// user may have typed into the buffer during the round trip, and
/// clobbering those keystrokes to complete a recovery binding would violate
/// the Prime Directive. When the version has moved on, `DocDb` is still
/// installed (this document's own recovery journal is real and should be
/// used going forward), but the buffer bytes are left exactly as the user
/// last typed them — this session's baseline simply anchors from the disk
/// content `load_document`'s caller already read, same as `recovered ==
/// disk_content` would.
pub fn handle_load_ack(
    app: &mut App,
    id: DocumentId,
    load_result: LoadResult,
    issued_version: Option<u64>,
) {
    let Some(expect_obs) = load_result.saved_obs else {
        app.set_status(
            "crash recovery unavailable for this tab: load returned no baseline observation",
            StatusSource::Other,
        );
        return;
    };

    let refusal = {
        let Some(doc) = app.doc_mut(id) else { return };
        if issued_version == Some(doc.buffer.version()) {
            match doc.hydrate(&load_result.disk_content, &load_result.recovered) {
                crate::document::Hydration::Refused(reason) => Some(reason),
                crate::document::Hydration::NoChange | crate::document::Hydration::Adopted => None,
            }
        } else {
            None
        }
    };
    if let Some(reason) = refusal {
        app.set_status(format!("crash recovery: {reason}"), StatusSource::Other);
    }

    let Some(doc) = app.doc_mut(id) else { return };
    doc.db = Some(DocDb::new(
        load_result.doc_id,
        expect_obs,
        false, // bind_new: `id` is already bound to a path read straight off disk
        load_result.bridge_seq.unwrap_or(0),
    ));
}

/// Records that `seq` was durably committed for `id`'s oldest still-pending
/// `AppendEdit` — called from `app::handle_db_event`'s `Msg::Db` handler on
/// `DbEvent::Ok { result: OpOutcome::Seq(seq), .. }`, after `app.db_ops` has
/// already resolved the ack's op id to `id`. `id` no longer live (an ack
/// racing a future close) is a correct, silent drop — the document it would
/// have updated is already gone.
pub fn resolve_append_ack(app: &mut App, id: DocumentId, seq: i64) {
    let Some(doc) = app.doc_mut(id) else { return };
    if let Some(doc_db) = doc.db.as_mut() {
        doc_db.resolve_append_ack(seq);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_db::{ClockFn, OpOutcome};
    use rune_vfs::{Mem, Vfs};

    fn in_memory_db() -> Db {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let store = Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
        let bridge = DbBridge::bootstrap();
        Db::new(store, bridge, false)
    }

    /// Plan WP1.S8: two documents each enqueue an `AppendEdit`; delivering
    /// their `DbEvent::Ok` acks (identified only by op id, via `app.db_ops`)
    /// must route each `Seq` result to the CORRECT document's `DocDb`, never
    /// crossing them.
    #[test]
    fn db_event_acks_route_to_the_correct_document_via_db_ops() {
        let mut app = App::new(
            Buffer::new("a"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id_a = app.active;
        let id_b = app.open_document(Buffer::new("b"));

        app.doc_mut(id_a).expect("doc a exists").db = Some(DocDb::new(1, 0, true, 0));
        app.doc_mut(id_b).expect("doc b exists").db = Some(DocDb::new(2, 0, true, 0));

        append_edit(&mut app, id_a, 1, &[], &[], &[]);
        append_edit(&mut app, id_b, 1, &[], &[], &[]);

        assert_eq!(app.db_ops.len(), 2);
        let op_for_a = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id_a)
            .expect("op recorded for doc a")
            .0;
        let op_for_b = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id_b)
            .expect("op recorded for doc b")
            .0;
        assert_ne!(op_for_a, op_for_b);

        // Simulate the acks arriving in reverse enqueue order — routing
        // must key off the op id, not arrival order.
        let doc_for_b = app.db_ops.remove(&op_for_b).expect("routes to doc b").doc;
        resolve_append_ack(&mut app, doc_for_b, 42);
        let doc_for_a = app.db_ops.remove(&op_for_a).expect("routes to doc a").doc;
        resolve_append_ack(&mut app, doc_for_a, 7);

        assert_eq!(
            app.doc(id_a)
                .expect("doc a exists")
                .db
                .as_ref()
                .expect("doc a has a DocDb")
                .last_known_seq,
            7
        );
        assert_eq!(
            app.doc(id_b)
                .expect("doc b exists")
                .db
                .as_ref()
                .expect("doc b has a DocDb")
                .last_known_seq,
            42
        );
        assert!(app.db_ops.is_empty());
    }

    #[test]
    fn handle_db_event_ok_seq_pops_db_ops_and_routes_to_the_right_document() {
        let mut app = App::new(
            Buffer::new("a"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id_a = app.active;
        let id_b = app.open_document(Buffer::new("b"));
        app.doc_mut(id_a).expect("doc a exists").db = Some(DocDb::new(1, 0, true, 0));
        app.doc_mut(id_b).expect("doc b exists").db = Some(DocDb::new(2, 0, true, 0));

        append_edit(&mut app, id_a, 1, &[], &[], &[]);
        let op_for_a = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id_a)
            .expect("op recorded for doc a")
            .0;

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(DbEvent::Ok {
                id: op_for_a,
                result: OpOutcome::Seq(99),
            }),
            &mut effects,
        );

        assert!(
            !app.db_ops.contains_key(&op_for_a),
            "a resolved ack must be popped from db_ops"
        );
        assert_eq!(
            app.doc(id_a)
                .expect("doc a exists")
                .db
                .as_ref()
                .expect("doc a has a DocDb")
                .last_known_seq,
            99
        );
    }

    /// Review fix: a `DbEvent::Fatal` tears the whole writer thread down —
    /// every `db_ops` entry still in flight will never receive its ack, so
    /// `handle_db_event`'s `Fatal` arm must clear the map outright rather
    /// than leaving those entries as dead weight for the rest of the
    /// session.
    #[test]
    fn handle_db_event_fatal_clears_every_in_flight_db_op() {
        let mut app = App::new(
            Buffer::new("a"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id_a = app.active;
        let id_b = app.open_document(Buffer::new("b"));
        app.doc_mut(id_a).expect("doc a exists").db = Some(DocDb::new(1, 0, true, 0));
        app.doc_mut(id_b).expect("doc b exists").db = Some(DocDb::new(2, 0, true, 0));

        append_edit(&mut app, id_a, 1, &[], &[], &[]);
        append_edit(&mut app, id_b, 1, &[], &[], &[]);
        assert_eq!(app.db_ops.len(), 2, "test setup: two ops in flight");

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(DbEvent::Fatal {
                error: "writer thread died".to_string(),
            }),
            &mut effects,
        );

        assert!(
            app.db_ops.is_empty(),
            "a Fatal event must clear every in-flight db_ops entry"
        );
        assert!(
            app.db.as_ref().expect("store still present").degraded,
            "a Fatal event must still degrade the store via on_store_failure"
        );
    }
}
