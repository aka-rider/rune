use rune_db::{ObsId, Seq};

use crate::db::PublishMode;

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
    pub last_known_seq: Seq,
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
    pub fn new(db_id: i64, publish_mode: PublishMode, last_known_seq: Seq) -> DocDb {
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
    pub(crate) fn resolve_append_ack(&mut self, seq: Seq) {
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
