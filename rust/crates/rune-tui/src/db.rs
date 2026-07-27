//! Wiring between `rune-tui`'s Elm-style runtime and `rune-db`'s async
//! writer-thread `Store` (plan WP5): the `DbEvent` -> `Msg::Db` bridge, the
//! per-`App` `AppDb` handle, and the small bookkeeping the three journal
//! call sites (`commands::edit::commit_edit_batch`/`undo`/`redo`) need to
//! talk to it. CONSTITUTION §1.4.8/§5.4: the in-memory
//! `rune_core::undo::Journal` stays the synchronous, authoritative source
//! of truth for the running session — nothing here ever waits on a `Store`
//! ack before mutating the buffer (plan decision 3), and every call below
//! is a plain, non-blocking channel send (`Store::enqueue`'s `try_send`),
//! never I/O — so these are called directly from `update`, not from a
//! spawned `Cmd`.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;
use rune_db::{DbEvent, ObsId, OnEvent, Store};

use crate::app::{self, App};
use crate::runtime::Msg;

/// Where a `DbEvent` goes before vs after the runtime loop exists. See
/// [`DbBridge`]'s doc comment for why this indirection is necessary at all.
enum Sink {
    Bootstrap(Sender<DbEvent>),
    Live(Sender<Msg>),
}

/// Adapts every `DbEvent` the `rune-db` writer thread posts into this
/// crate's `Msg` channel. Constructed once at bootstrap (`rune-cli::main`),
/// BEFORE `runtime::run` creates its own `Sender<Msg>` (plan Gotchas: "the
/// runtime never exposes its `Sender<Msg>`" — `runtime.rs:66` creates it
/// privately) — `Store::open`/`open_in_memory` (also bootstrap-time, so
/// hydration, WP5.S4, can finish before the TUI ever draws a frame) take
/// their `on_event` callback fixed at construction, with no way to swap it
/// afterward.
///
/// Bootstrap hydration polls the paired [`mpsc::Receiver<DbEvent>`]
/// directly with a blocking `recv`, filtering for the `Load` op's own id;
/// `runtime::run` then calls [`DbBridge::attach`] exactly once, at the very
/// top of the loop (mirroring how it seeds the initial `Msg::Resize`
/// through the ordinary `update` path rather than a one-off field write —
/// this is WP5.S1's "App-held setter"), so every LATER `DbEvent` is
/// delivered as `Msg::Db` through the normal Elm loop instead.
pub struct DbBridge {
    sink: Mutex<Sink>,
}

impl DbBridge {
    /// Constructs a bridge in its `Bootstrap` state, returning the paired
    /// receiver bootstrap hydration blocks on.
    pub fn bootstrap() -> (Arc<DbBridge>, mpsc::Receiver<DbEvent>) {
        let (tx, rx) = mpsc::channel();
        (
            Arc::new(DbBridge {
                sink: Mutex::new(Sink::Bootstrap(tx)),
            }),
            rx,
        )
    }

    /// The `Store::open`/`open_in_memory` `on_event` callback.
    pub fn on_event(self: &Arc<Self>) -> OnEvent {
        let bridge = Arc::clone(self);
        Box::new(move |evt| bridge.deliver(evt))
    }

    fn deliver(&self, evt: DbEvent) {
        let sink = self.sink.lock().unwrap_or_else(|p| p.into_inner());
        match &*sink {
            Sink::Bootstrap(tx) => {
                let _ = tx.send(evt);
            }
            Sink::Live(tx) => {
                let _ = tx.send(Msg::Db(evt));
            }
        }
    }

    /// Switches the bridge to `Live`: every subsequent `DbEvent` is wrapped
    /// as `Msg::Db(...)` and delivered through `tx` instead.
    pub fn attach(&self, tx: Sender<Msg>) {
        *self.sink.lock().unwrap_or_else(|p| p.into_inner()) = Sink::Live(tx);
    }
}

/// This `App`'s handle onto its recovery store: the `Store` itself, the
/// bridge routing its acks into `Msg::Db`, this session's bound document,
/// and the small amount of bookkeeping the async replica needs. One `App`
/// == one document == one `AppDb` (Phase 1 is one file, one pane).
pub struct AppDb {
    pub store: Store,
    pub bridge: Arc<DbBridge>,
    pub doc_id: i64,
    /// True once this store can no longer be trusted for recovery — either
    /// the open ladder degraded to `:memory:` at launch, or a LATER
    /// enqueue-time error / `DbEvent::Err`/`Fatal` (plan decision 3: "on
    /// hard write failure the buffer is never rolled back; the store
    /// enters degraded mode"). Sticky for the process lifetime — WP5 has no
    /// reopen/reconnect path.
    pub degraded: bool,
    /// This session's current CAS baseline for `doc_id` — updated from
    /// every successful `materialize` ack's `saved` observation (plan
    /// WP5.S6). Seeded from `LoadResult::saved_obs` at hydration.
    pub expect_obs: ObsId,
    /// Whether the NEXT save must go through `materialize`'s `bind_new`
    /// (create-only, `rename_excl`) path rather than the CAS-overwrite path
    /// — true until the first successful create commits.
    pub bind_new: bool,
    /// The highest durable journal seq (`events.seq`) this session has SEEN
    /// acknowledged so far — a conservative stand-in for "the durable
    /// journal's current head", used as `materialize`'s `seq` parameter and
    /// as the fallback when `seq_by_local_pos` doesn't have an exact answer
    /// yet (see its doc comment).
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
    /// `AppendEdit` acks land in the same relative order their ops were
    /// enqueued, so the oldest entry here is always the next ack to fill
    /// in.
    pub pending_seq_acks: VecDeque<usize>,
    /// Bumped on every journal mutation; the debounce token for the 2s
    /// snapshot-autosave timer (plan WP5.S6, port of
    /// `workspace_timers.go:11`) — a `Msg::SnapshotDue` arriving with a
    /// stale generation means a later edit already superseded it, so it's
    /// ignored.
    pub snapshot_generation: u32,
}

impl AppDb {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Store,
        bridge: Arc<DbBridge>,
        doc_id: i64,
        degraded: bool,
        expect_obs: ObsId,
        bind_new: bool,
        last_known_seq: i64,
    ) -> AppDb {
        AppDb {
            store,
            bridge,
            doc_id,
            degraded,
            expect_obs,
            bind_new,
            last_known_seq,
            seq_by_local_pos: Vec::new(),
            pending_seq_acks: VecDeque::new(),
            snapshot_generation: 0,
        }
    }

    /// Deterministically drains and joins the underlying `Store`'s
    /// writer/reader threads (`Store::shutdown`'s own doc comment) — the
    /// one place `rune-cli::main` closes the recovery store on every exit
    /// path, not just its own bootstrap-failure branches.
    pub fn shutdown(self) {
        self.store.shutdown();
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
/// to the LOCAL in-memory journal (plan WP5.S3) — called immediately after
/// `Journal::push` at `commands::edit::commit_edit_batch`'s one call site.
/// `local_pos` is `app.editor.journal.pos()` AFTER that push. A failure
/// here (enqueue-time `Error`, never an async one — that lands via
/// `Msg::Db` instead) only ever marks the store degraded
/// (`app::on_store_failure`) — the buffer/journal mutation already
/// happened and is never rolled back (plan decision 3).
pub fn append_edit(
    app: &mut App,
    local_pos: usize,
    edits: &[AppliedEdit],
    cursors_before: &[Cursor],
    cursors_after: &[Cursor],
) {
    let Some(db) = app.db.as_ref() else { return };
    if db.degraded {
        return;
    }
    let result = db
        .store
        .append_edit(db.doc_id, edits, cursors_before, cursors_after);
    match result {
        Ok(_op_id) => {
            if let Some(db) = app.db.as_mut() {
                db.note_pending_append(local_pos);
            }
        }
        Err(e) => app::on_store_failure(app, e.to_string()),
    }
}

/// Enqueues a `MoveUndoPos` replica of an undo/redo this session just
/// committed locally (plan WP5.S3) — called immediately after
/// `Journal::move_pos` at `commands::edit::undo`/`redo`'s call sites.
/// `local_pos` is the journal position just committed (`Journal::move_pos`'s
/// own argument).
pub fn move_undo_pos(app: &mut App, local_pos: usize) {
    let Some(db) = app.db.as_ref() else { return };
    if db.degraded {
        return;
    }
    let target_seq = db.seq_for_local_pos(local_pos);
    let result = db.store.move_undo_pos(db.doc_id, target_seq);
    if let Err(e) = result {
        app::on_store_failure(app, e.to_string());
    }
}

/// Records that `seq` was durably committed for the oldest still-pending
/// `AppendEdit` — called from `app::update`'s `Msg::Db` handler on
/// `DbEvent::Ok { result: OpOutcome::Seq(seq), .. }`.
pub fn resolve_append_ack(app: &mut App, seq: i64) {
    if let Some(db) = app.db.as_mut() {
        db.resolve_append_ack(seq);
    }
}
