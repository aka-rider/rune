//! Wiring between `rune-tui`'s Elm-style runtime and `rune-db`'s async
//! writer-thread `Store` (plan WP5, re-split in WP1 for multi-document
//! support — plan WP1 decision 5): the `DbEvent` -> `Msg::Db` bridge, the
//! app-level `Db` handle (the `Store` itself + the bridge + the sticky
//! degraded flag), and the per-document `DocDb` handle (this doc's bound
//! row plus its async-replica bookkeeping). The
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
    /// `Some(save_epoch)` iff this op is a `Probe` — the issuing document's
    /// `DocDb::save_epoch` at enqueue time. The `OpOutcome::Sync` ack
    /// handler compares this against `DocDb::save_epoch` as it stands when
    /// the reply lands: a materialize publishing in between means the disk
    /// this probe read is no longer the current one, and the reply is
    /// dropped rather than trusted — the same generation-echo shape
    /// `merge_gen` below uses, scoped to one document's own save lineage
    /// instead of a single app-wide counter.
    pub probe_epoch: Option<u32>,
    /// `Some(generation)` iff this op is a `MergePrep` (plan WP3.S1/S5) — the
    /// generation `merge::begin` minted for this attempt at enqueue time.
    /// The landing handler (`merge::handle_merge_prep_ack`) compares this
    /// against `App.merge`'s OWN current `Pending` generation for the same
    /// document before trusting the ack: a later `^M` superseding this
    /// attempt before it lands must not have this stale ack mistaken for
    /// the current one (mirrors `is_probe`'s in-flight bookkeeping, but a
    /// generation counter rather than a plain flag, since more than one
    /// merge attempt can be in flight in sequence for the same document).
    pub merge_gen: Option<u32>,
    /// True iff this `Load` was issued to re-baseline an already-live
    /// document's `DocDb` (the save-ack re-baseline in `materialize_ack::
    /// reactions`, and the lost-create-race hand-off) rather than to
    /// recover a document the user just opened. A recovery `Load` is
    /// meant to adopt whatever the store recovered; a re-baseline `Load`
    /// exists only to install a fresh CAS baseline against content this
    /// session already knows about, so `db_ack::handle_load_ack` must
    /// never let it hydrate the buffer — doing so would silently replace
    /// the user's own typing with a stale recovery row's content the
    /// instant the round trip lands.
    pub binding_only: bool,
    /// True iff this op only READS one document's disk/journal state
    /// (`Probe`/`Load`/`MergePrep`): its failure means "this document's
    /// read didn't land", never "the store can't be trusted for recovery",
    /// so the `DbEvent::Err` router posts a per-document error instead of
    /// sticky-degrading the whole store.
    pub doc_scoped: bool,
}

impl PendingOp {
    pub fn new(doc: DocumentId) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            mints_scratch: false,
            is_probe: false,
            probe_epoch: None,
            merge_gen: None,
            binding_only: false,
            doc_scoped: false,
        }
    }

    pub fn load(doc: DocumentId, issued_version: u64, binding_only: bool) -> PendingOp {
        PendingOp {
            doc,
            issued_version: Some(issued_version),
            mints_scratch: false,
            is_probe: false,
            probe_epoch: None,
            merge_gen: None,
            binding_only,
            doc_scoped: true,
        }
    }

    pub fn create_scratch(doc: DocumentId) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            mints_scratch: true,
            is_probe: false,
            probe_epoch: None,
            merge_gen: None,
            binding_only: false,
            doc_scoped: false,
        }
    }

    pub fn probe(doc: DocumentId, save_epoch: u32) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            mints_scratch: false,
            is_probe: true,
            probe_epoch: Some(save_epoch),
            merge_gen: None,
            binding_only: false,
            doc_scoped: true,
        }
    }

    pub fn merge_prep(doc: DocumentId, generation: u32) -> PendingOp {
        PendingOp {
            doc,
            issued_version: None,
            mints_scratch: false,
            is_probe: false,
            probe_epoch: None,
            merge_gen: Some(generation),
            binding_only: false,
            doc_scoped: true,
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
/// `DocumentId`, the in-process tab identity) and the async-replica
/// bookkeeping reconciling the LOCAL journal against the DURABLE one. The
/// CAS baseline itself — `expect_obs`/`pending_rebaseline_hash`/`save_epoch`
/// — lives on [`FileBinding`], shared by every `Document` bound to the same
/// `db_id`, never copied here: two tabs opened onto the SAME underlying file
/// must see the one truth about what disk holds, or one tab's own save
/// falsely raises the disk-conflict guard against the other's very next
/// attempt.
pub struct DocDb {
    pub db_id: i64,
    /// Whether the NEXT save must go through `materialize`'s `bind_new`
    /// (create-only, `rename_excl`) path rather than the CAS-overwrite path
    /// — true until the first successful create commits. Never shared: a
    /// scratch row a create is still racing to bind is, by construction,
    /// claimed by exactly one `Document`.
    pub bind_new: bool,
    /// The highest durable journal seq (`events.seq`) this session has SEEN
    /// acknowledged so far for this document — a conservative stand-in for
    /// "the durable journal's current head", used only as `materialize`'s
    /// `seq` parameter (a save's `observations.seq` tag — informational
    /// only, never read back to reconstruct content, so a lagging estimate
    /// here is stale metadata, not a correctness hazard). `MoveUndoPos`'s
    /// target seq and `CreateSnapshot`'s anchor seq are both resolved
    /// fresh, writer-side, at op-execution time instead of estimated from
    /// this field — see `rune_db::OpKind::MoveUndoPos`/`OpKind::
    /// CreateSnapshot`'s own doc comments for why an app-side estimate can
    /// never be exact for either.
    pub last_known_seq: i64,
    /// Bumped on every journal mutation; the debounce token for the 2s
    /// snapshot-autosave timer (plan WP5.S6) — a `Msg::SnapshotDue`
    /// arriving with a stale generation means a later edit already
    /// superseded it, so it's ignored.
    pub snapshot_generation: u32,
}

impl DocDb {
    pub fn new(db_id: i64, bind_new: bool, last_known_seq: i64) -> DocDb {
        DocDb {
            db_id,
            bind_new,
            last_known_seq,
            snapshot_generation: 0,
        }
    }

    /// Records the durable seq an `AppendEdit` ack just reported — kept only
    /// as a lagging estimate of "the durable journal's current head" for
    /// `materialize`'s informational `seq` tag (`last_known_seq`'s own doc
    /// comment); `MoveUndoPos`/`CreateSnapshot` no longer read this at all.
    pub(crate) fn resolve_append_ack(&mut self, seq: i64) {
        self.last_known_seq = self.last_known_seq.max(seq);
    }
}

/// This process's single CAS baseline for a store-bound file, shared by
/// EVERY `Document` currently bound to its `db_id` — the fix for the
/// false-conflict class where two tabs on one file each held an
/// independent, silently-diverging copy of `expect_obs`. Lives in
/// `App::file_bindings`, keyed by `db_id`; installed once, the moment the
/// FIRST document binds that `db_id` (`db_ack::bind_file`'s own doc
/// comment), and joined — never reseeded — by every later document binding
/// the same `db_id`, so a second tab opening the file adopts whatever the
/// first tab's own saves have already advanced it to rather than resetting
/// it from its own possibly-older `Load`. Pruned once no open `Document`
/// references `db_id` any longer (`App::prune_file_binding`).
pub struct FileBinding {
    /// This process's current CAS baseline for `db_id` — updated from every
    /// document's successful `materialize` ack's `saved` observation, and
    /// from a terminal merge/discard adoption
    /// (`merge::landing::advance_expect_obs`). Seeded from the first
    /// `LoadResult::saved_obs` this `db_id` ever saw.
    pub expect_obs: ObsId,
    /// Set when a write physically committed but the observation that would
    /// have advanced `expect_obs` was lost to a failing writer — `expect_obs`
    /// itself is left untouched (it may be the only row this session has
    /// ever recorded), so a save starting before the re-baseline `Load`
    /// lands would otherwise CAS-compare the disk against that stale row and
    /// manufacture a conflict against bytes a session just wrote. Holds
    /// the hash of exactly those bytes so such a save recognizes the disk as
    /// its own echo; disk content that disagrees with it still conflicts
    /// normally — this is never a license to adopt someone else's bytes.
    /// Cleared the moment a real observation lands again.
    pub pending_rebaseline_hash: Option<String>,
    /// This process's save epoch for `db_id` — bumped exactly once, inside
    /// `materialize_ack::handle_materialize_ack`'s committed branch, the
    /// moment ANY document bound to `db_id` gets a publish's
    /// `MaterializeRecord` ack. A `Probe` records this value onto its own
    /// `PendingOp` at issue time (`PendingOp::probe_epoch`'s own doc
    /// comment); the ack handler drops a reply whose recorded epoch no
    /// longer matches, since a publish landing in between — from ANY tab on
    /// this file — means the disk the probe read is stale. Shared exactly
    /// because the disk fact it echoes is a fact about the FILE, not about
    /// whichever tab happened to trigger the publish.
    pub save_epoch: u32,
    /// Set by `db_enqueue::probe` when a probe was skipped because a save
    /// was in flight — for ANY document bound to `db_id` — at the moment it
    /// was asked for; that save's publish invalidates whatever the disk
    /// looked like before it, so probing anyway would only end up dropped by
    /// the epoch check above. Consumed (taken and cleared) by
    /// `handle_materialize_ack`'s own tail once a save for `db_id` resolves
    /// — REGARDLESS of which tab's save it was — which then re-issues a
    /// fresh probe for every document still open on `db_id`, so the disk
    /// fact every one of them ends up with is read from the POST-save world,
    /// exactly once per document.
    pub pending_probe: bool,
}

impl FileBinding {
    pub fn new(expect_obs: ObsId) -> FileBinding {
        FileBinding {
            expect_obs,
            pending_rebaseline_hash: None,
            save_epoch: 0,
            pending_probe: false,
        }
    }
}

impl crate::app::App {
    /// Joins `db_id`'s shared [`FileBinding`], seeding it from
    /// `seed_expect_obs` only if no document has ever bound this `db_id`
    /// before — called exactly once per document, at the moment it installs
    /// its OWN `DocDb` for `db_id` (`db_ack::handle_load_ack`/`handle_
    /// create_scratch_ack`). A SECOND document binding the same `db_id`
    /// finds the entry already present and adopts it as-is: by the writer
    /// thread's own strict FIFO order, this document's fresh `Load` can
    /// never observe a baseline OLDER than what a sibling document's own
    /// earlier save already advanced the shared entry to, so joining rather
    /// than reseeding never regresses it.
    pub fn bind_file(&mut self, db_id: i64, seed_expect_obs: ObsId) {
        self.file_bindings
            .entry(db_id)
            .or_insert_with(|| FileBinding::new(seed_expect_obs));
    }

    pub fn file_binding(&self, db_id: i64) -> Option<&FileBinding> {
        self.file_bindings.get(&db_id)
    }

    pub fn file_binding_mut(&mut self, db_id: i64) -> Option<&mut FileBinding> {
        self.file_bindings.get_mut(&db_id)
    }

    /// `id`'s bound `db_id`, or `None` when the document has no store
    /// binding at all — the one place `doc(id).db.as_ref().map(|d| d.db_id)`
    /// is spelled out, so every caller that only needs the id (never the
    /// [`FileBinding`] itself, e.g. to prune it) shares this instead of
    /// re-deriving it by hand.
    pub fn doc_db_id(&self, id: DocumentId) -> Option<i64> {
        self.doc(id).and_then(|d| d.db.as_ref().map(|d| d.db_id))
    }

    /// `id`'s shared [`FileBinding`] — `None` when `id` has no store binding
    /// (an untitled/unbound document) or, as an internal-inconsistency
    /// case that should never occur, when it does but no entry was ever
    /// joined for its `db_id`. The one chokepoint for "this document's
    /// store binding, then its shared per-file baseline" — every caller
    /// that used to hand-roll `doc(id).db.as_ref().map(...).and_then(|db_id|
    /// file_binding(db_id))` shares this instead.
    pub fn doc_file_binding(&self, id: DocumentId) -> Option<&FileBinding> {
        self.file_binding(self.doc_db_id(id)?)
    }

    /// [`Self::doc_file_binding`]'s mutable counterpart.
    pub fn doc_file_binding_mut(&mut self, id: DocumentId) -> Option<&mut FileBinding> {
        let db_id = self.doc_db_id(id)?;
        self.file_binding_mut(db_id)
    }

    /// Removes `db_id`'s shared baseline once no open `Document` references
    /// it any longer — called after every transition that can leave a
    /// `db_id` unreferenced (a close, or a document dropping its own `db`
    /// binding entirely), so a long session opening and closing many files
    /// never grows `file_bindings` unboundedly. A no-op while at least one
    /// document still names `db_id`.
    pub fn prune_file_binding(&mut self, db_id: i64) {
        let still_referenced = self
            .documents
            .values()
            .any(|d| d.db.as_ref().is_some_and(|db| db.db_id == db_id));
        if !still_referenced {
            self.file_bindings.remove(&db_id);
        }
    }

    /// Whether ANY document currently bound to `db_id` has a save in
    /// flight — the shared-file counterpart to a single document's own
    /// `save_in_flight`: a probe against a file another tab is mid-publish
    /// to would only read a soon-to-be-stale disk state and get dropped by
    /// the epoch check anyway (`db_enqueue::probe`'s own doc comment).
    pub fn any_save_in_flight_for(&self, db_id: i64) -> bool {
        self.documents
            .values()
            .any(|d| d.db.as_ref().is_some_and(|db| db.db_id == db_id) && d.save_in_flight)
    }

    /// Every currently-open document bound to `db_id` — used to re-issue a
    /// deferred probe for every tab a save's completion just unblocked
    /// (`materialize_ack::handle_materialize_ack`'s tail), not merely the
    /// one document whose OWN save happened to resolve.
    pub fn documents_bound_to(&self, db_id: i64) -> Vec<DocumentId> {
        self.documents
            .iter()
            .filter(|(_, d)| d.db.as_ref().is_some_and(|db| db.db_id == db_id))
            .map(|(&id, _)| id)
            .collect()
    }
}
