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
    pub merge_gen: Option<crate::generation::Generation>,
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

    pub fn merge_prep(doc: DocumentId, generation: crate::generation::Generation) -> PendingOp {
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

pub struct DocDb {
    pub db_id: i64,
    /// Never shared: a scratch row a create is still racing to bind is, by
    /// construction, claimed by exactly one `Document`.
    pub publish_mode: PublishMode,
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
    pub last_known_seq: rune_db::Seq,
    /// Bumped on every journal mutation; the debounce token for the 2s
    /// snapshot-autosave timer — a `Msg::SnapshotDue`
    /// arriving with a stale generation means a later edit already
    /// superseded it, so it's ignored.
    pub snapshot_generation: u32,
    /// The writer thread numbers this binding's local undo positions by
    /// the `AppendEdit`s it actually ran, starting over at every bind that
    /// is not a preserved same-row re-baseline —
    /// `writer position = local journal position - undo_offset`.
    /// `db_ack`'s install computes this from what the two sides genuinely
    /// disagree by: an adopting hydration's synthetic bridge `Step` lands
    /// in the local journal but never reaches the writer (+1); a re-base
    /// bridge the install itself enqueues reaches the writer but never the
    /// local journal (-1); a hand-off rebind restarts the writer's
    /// numbering under a journal that keeps counting (+pos).
    pub(crate) undo_offset: i64,
    /// The lowest writer position `undo_offset`'s mapping is valid for. A
    /// local undo resolving below it names a buffer state the bound row's
    /// journal cannot express — everything before a rebind's re-base
    /// bridge — so `db_enqueue::move_undo_pos` must never send it as an
    /// exact position (that mis-resolves into another lineage's seq and
    /// truncates or resurrects journal rows); it journals a forward
    /// re-base instead.
    pub(crate) undo_floor: i64,
    /// How many `AppendEdit`s this session has enqueued against this
    /// binding since it was installed — the upper bound of the writer
    /// positions that exist at all, maintained by `db_enqueue::send_append`
    /// and seeded by `db_ack::bind_document_row` with the appends already
    /// in flight when a same-row re-baseline restarted the writer's
    /// numbering (carried across verbatim when it did not). A resolution
    /// above it names an entry the writer has never run (a redo past a
    /// re-base), which — like one below
    /// `undo_floor` — must be journaled as a forward re-base, never sent.
    pub(crate) appends_sent: i64,
    /// A re-base bridge `db_ack::bind_document_row` computed but has not
    /// journaled yet: the replace-all step turning what the bound row
    /// currently reconstructs to into the content this document's next
    /// `AppendEdit`'s coordinates assume. Deferred rather than journaled at
    /// bind time because journaling it rewrites the row's reconstruction —
    /// a re-baseline bind that is never edited must leave a dead
    /// session's recovered draft (and its resumable merge) reconstructable.
    /// The deferral is safe for the USER'S OWN words only because a
    /// hand-off rebind's abandoned scratch row still holds every pre-rebind
    /// keystroke as a recoverable draft — the buffer content the new row
    /// cannot yet reconstruct is durably held by the old one until the
    /// bridge lands. Flushed by `db_enqueue::flush_pending_rebase`
    /// immediately before the first op whose meaning depends on the
    /// reconstruction matching the buffer: an `AppendEdit`, a durable undo
    /// move, a save, a snapshot.
    pub(crate) pending_rebase: Option<crate::document::ReplicaStep>,
}

impl DocDb {
    pub fn new(db_id: i64, publish_mode: PublishMode, last_known_seq: rune_db::Seq) -> DocDb {
        DocDb {
            db_id,
            publish_mode,
            last_known_seq,
            snapshot_generation: 0,
            undo_offset: 0,
            undo_floor: 0,
            appends_sent: 0,
            pending_rebase: None,
        }
    }

    /// Records the durable seq an `AppendEdit` ack just reported — kept only
    /// as a lagging estimate of "the durable journal's current head" for
    /// `materialize`'s informational `seq` tag (`last_known_seq`'s own doc
    /// comment); `MoveUndoPos`/`CreateSnapshot` no longer read this at all.
    pub(crate) fn resolve_append_ack(&mut self, seq: rune_db::Seq) {
        self.last_known_seq = self.last_known_seq.max(seq);
    }
}

/// This process's single CAS baseline for a store-bound file, shared by
/// EVERY `Document` currently bound to its `db_id` — the fix for the
/// false-conflict class where two tabs on one file each held an
/// independent, silently-diverging copy of `expect_obs`. Lives in
/// `App::file_bindings`, keyed by `db_id`; installed once, the moment the
/// FIRST document binds that `db_id` (`App::install_or_join_file_binding`'s
/// own doc comment), and joined — never reseeded — by every later document binding
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
    pub expect_obs: Option<ObsId>,
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
    /// This process's baseline epoch for `db_id` — bumped whenever the
    /// session itself rewrites the file's reconciliation baseline: a
    /// publish's `MaterializeRecord` ack (`materialize_ack::
    /// handle_materialize_ack`'s committed branch), a merge attempt's
    /// terminal success (`merge::landing::advance_expect_obs` — a Discard
    /// or no-conflict install, or a completed resolution), and an abandoned
    /// merge's resolve retraction (`merge::enqueue_resolve_abandon`). A
    /// `Probe` records this value onto its own `PendingOp` at issue time
    /// (`PendingOp::baseline_epoch`'s own doc comment); the ack handler drops
    /// a reply whose recorded epoch no longer matches, since a baseline
    /// rewrite landing in between — from ANY tab on this file — means the
    /// verdict the probe computed is stale, and re-probes so the fresh
    /// verdict is read from the post-rewrite world. Shared exactly because
    /// the baseline it echoes is a fact about the FILE, not about whichever
    /// tab happened to rewrite it.
    pub baseline_epoch: u32,
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
    pub fn new(expect_obs: Option<ObsId>) -> FileBinding {
        FileBinding {
            expect_obs,
            pending_rebaseline_hash: None,
            baseline_epoch: 0,
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
    pub fn install_or_join_file_binding(&mut self, db_id: i64, seed_expect_obs: Option<ObsId>) {
        if let std::collections::hash_map::Entry::Vacant(vacant) = self.file_bindings.entry(db_id) {
            vacant.insert(FileBinding::new(seed_expect_obs));
        }
    }

    /// Advances `db_id`'s shared [`FileBinding`] to `obs` unconditionally —
    /// the re-baseline counterpart to [`App::install_or_join_file_binding`],
    /// called only from a `Rebaseline` `Load` ack (`db_ack::
    /// handle_load_ack`). When `db_id` already has a binding, always
    /// overwrites `expect_obs` and clears `pending_rebaseline_hash`, even
    /// though the ordinary join path never would: a re-baseline exists
    /// precisely to correct a baseline that path left stale.
    ///
    /// A missing entry is NOT an inconsistency — the lost-create-race
    /// hand-off (`materialize_ack::reactions`) enqueues a `Rebaseline`
    /// `Load` against the RACER's own row, a `db_id` this process may never
    /// have touched before, so its first-ever sighting legitimately lands
    /// here rather than through [`App::install_or_join_file_binding`]. That
    /// case installs a fresh binding from `obs`, exactly like the ordinary
    /// join path's own first install would.
    pub fn rebaseline_file_binding(&mut self, db_id: i64, obs: ObsId) {
        self.file_bindings
            .entry(db_id)
            .and_modify(|binding| {
                binding.expect_obs = Some(obs);
                binding.pending_rebaseline_hash = None;
            })
            .or_insert_with(|| FileBinding::new(Some(obs)));
    }

    pub fn file_binding(&self, db_id: i64) -> Option<&FileBinding> {
        self.file_bindings.get(&db_id)
    }

    pub fn file_binding_mut(&mut self, db_id: i64) -> Option<&mut FileBinding> {
        self.file_bindings.get_mut(&db_id)
    }

    /// `id`'s bound `db_id`, or `None` when the document has no store
    /// binding at all — the one place `doc(id).doc_db().map(|d| d.db_id)`
    /// is spelled out, so every caller that only needs the id (never the
    /// [`FileBinding`] itself, e.g. to prune it) shares this instead of
    /// re-deriving it by hand.
    pub fn doc_db_id(&self, id: DocumentId) -> Option<i64> {
        self.doc(id).and_then(|d| d.doc_db().map(|d| d.db_id))
    }

    /// `id`'s shared [`FileBinding`] — `None` when `id` has no store binding
    /// (an untitled/unbound document) or, as an internal-inconsistency
    /// case that should never occur, when it does but no entry was ever
    /// joined for its `db_id`. The one chokepoint for "this document's
    /// store binding, then its shared per-file baseline" — every caller
    /// that used to hand-roll `doc(id).doc_db().map(...).and_then(|db_id|
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
            .any(|d| d.doc_db().is_some_and(|db| db.db_id == db_id));
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
            .any(|d| d.doc_db().is_some_and(|db| db.db_id == db_id) && d.save_in_flight())
    }

    /// Every currently-open document bound to `db_id` — used to re-issue a
    /// deferred probe for every tab a save's completion just unblocked
    /// (`materialize_ack::handle_materialize_ack`'s tail), not merely the
    /// one document whose OWN save happened to resolve.
    pub fn documents_bound_to(&self, db_id: i64) -> Vec<DocumentId> {
        self.documents
            .iter()
            .filter(|(_, d)| d.doc_db().is_some_and(|db| db.db_id == db_id))
            .map(|(&id, _)| id)
            .collect()
    }
}
