//! `DocumentId` + `Document`: one open editing pane's full state — buffer,
//! cursors, the display-pipeline root machine, the scrollable viewport onto
//! it, file identity, save/dirty bookkeeping, and its own recovery-store
//! handle — fat `Document`, no separate View type: `Document`
//! absorbs everything the previous `Editor` held plus every per-doc field
//! that used to live directly on `App`. `Document::sync` is the fixed
//! per-message sync sequence: `sync_content`
//! iff version changed -> `set_width` -> `sync_cursors` -> `snapshot` ->
//! scroll-to-cursor -> re-`view` -> scroll-to-cursor again -> re-`view` once
//! more, since a scroll command can move the cursor itself, and that move
//! can itself change reveal-driven display geometry the first reconcile
//! already settled against — see `sync`'s own docs).
//!
//! Split for the 500-line budget: this file holds the type itself plus
//! construction/identity/hydration/save-state bookkeeping; [`sync`] holds
//! the view/scroll/settle sequence (`view`, `sync_catalogue`,
//! `scroll_to_cursor`, `sync`) as a second `impl Document` block, since none
//! of that sequence depends on anything declared only in this file.

mod graphics;
mod replica;
mod save_state;
mod sync;
#[cfg(test)]
mod tests;

pub use crate::read_only::ReadOnly;
pub(crate) use replica::{Replica, ReplicaStep};
pub(crate) use save_state::PublishParams;
pub use save_state::{SavePhase, SaveTicket};
use save_state::{SaveState, SaveTicketMint};

use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use rune_core::buffer::{Buffer, Edit, SortedEdits};
use rune_core::cursor::{Cursor, CursorSet};
use rune_core::undo::{EditKind, Journal, Step};
use rune_md::element::doc::{DocMachine, ViewSnapshots};
use rune_md::icons::IconSet;
use rune_syntax::DocumentKind;
use rune_syntax::element::ByteRange;

use crate::db::DocDb;
pub use crate::document_support::Hydration;
use crate::document_support::{is_suspicious_shrink, kind_for};
use crate::highlight::HighlightState;
use crate::undogroup::Direction;
use crate::viewport::Viewport;

/// Identifies one open `Document` for the lifetime of the process — minted
/// monotonically by `App::next_doc_id`. Tabs and every
/// doc-scoped `Msg` key on this, never on a path: help/untitled documents
/// are first-class and have no path at all. The inner `NonZeroU64` is
/// `pub(crate)`, not private: `App` — the sole minter, via
/// `App::mint_doc_id` — constructs one directly from its own `NonZeroU64`
/// counter, with no fallible conversion step to route around.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DocumentId(pub(crate) NonZeroU64);

/// One open editing pane's complete state: buffer,
/// cursors, the root display machine, the viewport onto it, file identity,
/// save/dirty bookkeeping, and this doc's own recovery-store handle.
/// `quit`/`messages`/`db_banner`/`should_quit` stay on `App`
/// (app-wide, not per-document); `pending_save_confirm` also stays on `App`
/// but is doc-tagged (`Option<(DocumentId, Generation)>`) so a tab switch can't
/// misapply an armed confirm gate.
pub struct Document {
    pub buffer: Buffer,
    pub cursors: CursorSet,
    pub doc: DocMachine,
    pub viewport: Viewport,
    pub focused: bool,
    /// Whether reveal may key off the cursor — deliberately separate from
    /// the caret gate `focused` feeds: an open search bar blurs the caret
    /// (keystrokes land in the bar, not the buffer), but its Enter/Shift+
    /// Enter navigation still drives THIS document's cursor, so a jump into
    /// a concealed element must still reveal it. Pushed down by
    /// `App::sync_view`, the same chokepoint that pushes `focused`.
    pub reveal_engaged: bool,
    /// The in-memory undo/redo journal: every applied edit batch is
    /// pushed here as a `Step`; `commands::edit::undo`/`redo` peek-then-
    /// commit against it.
    pub journal: Journal,
    pub(crate) ladder_presses: usize,
    pub(crate) ladder_pressed_at: Option<Instant>,
    pub(crate) ladder_direction: Option<Direction>,
    pub(crate) ladder_anchor: Option<usize>,
    /// Guards every buffer-mutating command (typing, backspace/delete,
    /// indent/outdent, cut, paste — anything that reaches
    /// `commands::edit::commit_edit_batch`, the sole writer of buffer
    /// mutations) against touching a read-only document. Checked at that ONE
    /// chokepoint rather than at each command's call site, so no future
    /// mutating command can forget the guard (an earlier
    /// version checked this only in `commands::clipboard::handle_paste_
    /// content`, leaving Cut and every keyboard-insert path able to mutate a
    /// "read-only" document — a bug class reintroduced by guarding the
    /// wrong layer).
    /// `commands::edit::undo`/`redo` check this field but only against
    /// `ReadOnly::Reading`/`ReadOnly::Preview`, never via a blanket
    /// `is_read_only()`. `ReadOnly::Always` stays exempt from that guard,
    /// because `Always` means the document has no editable form at all
    /// (Help, an image, the error banner), not a view mode the user can
    /// leave.
    /// `Reading` is different: it is a toggle the user reaches and leaves
    /// with the same chord (⌃P), so undo/redo are blocked there while the
    /// toggle is on. The two chords diverge deliberately: in `Reading`,
    /// ^S still materializes bytes already typed, while ⌘Z (which would
    /// change them) does not — saving protects what the user wrote, undo
    /// rewrites it.
    pub read_only: ReadOnly,
    /// Marks the tab the tab-cap eviction victim search must skip
    /// (set/cleared by the ^G toggle).
    pub pinned: bool,
    /// The file this document is bound to, or `None` for an untitled draft
    /// (moved off `App`: every open document has its own identity).
    pub file_path: Option<PathBuf>,
    /// The buffer version the LAST successful save/materialize ack
    /// persisted — advanced ONLY from a store ack (`save::handle_materialize_
    /// ack`) or, for the no-store fallback path, `Msg::SaveDone` (see
    /// `save::trigger_save`'s docs), and only ever via [`Document::finish_save_ok`].
    /// `is_dirty` checks this FIRST, as a cheap short-circuit: only when it
    /// differs from the live buffer's version does the byte comparison
    /// below ever run.
    pub saved_version: u64,
    /// The bytes the last successful save/materialize ack actually
    /// persisted — ground truth dirtiness compares the LIVE
    /// buffer's content against THIS, not a version proxy alone: `Buffer::
    /// apply_edits` always returns `version + 1`, and undo/redo build a new
    /// buffer, so a version comparison alone leaves an edit-then-undo
    /// document dirty forever even though the bytes are back to identical.
    /// Advanced only by [`Document::finish_save_ok`]. `pub(crate)` (not
    /// `pub`): only `finish_save_ok` may move the saved baseline, and a
    /// `pub` field left that invariant enforced by convention rather than
    /// the type system alone — an out-of-crate integration test that needs
    /// a dirty fixture goes through a real edit instead, see
    /// `dirty_common::force_dirty`.
    pub(crate) saved_content: Arc<str>,
    /// The save-lifecycle state machine — `Idle`, or one attempt's own
    /// ticket/capture/publish bookkeeping, one owner at a time: `Direct`
    /// (the no-store fallback), `Preparing` (a `MaterializePrepare` is
    /// outstanding), `Publishing` (its vfs `Cmd` is outstanding — the store
    /// dying no longer releases this document while that write is still
    /// headed to disk), `Recording` (the `MaterializeRecord` bookkeeping
    /// ack is outstanding after the write already committed). Private —
    /// `begin_save`/`begin_prepare`/`begin_publishing`/`begin_recording`/
    /// `finish_save_ok`/`abandon_save` are the only places allowed to
    /// touch it, so a save can never be in flight without the exact
    /// ticket/bytes it captured, and a stale ticket can never be promoted
    /// by a later unrelated ack. `save_in_flight()` derives from this
    /// rather than caching a parallel bool; a second save attempt is
    /// refused outright while this is anything but `Idle`, so no App-level
    /// map can ever be overwritten by a second attempt's capture while the
    /// first one's own publish is still outstanding.
    save: SaveState,
    next_save_ticket: SaveTicketMint,
    /// The most recent display-pipeline snapshot, cached by `App::sync_view`
    /// for `render::draw` to blit. `None` only before this document's first
    /// sync.
    pub view: Option<ViewSnapshots>,
    /// This document's relationship to the app-wide recovery store —
    /// `Detached`/`Binding`/`Bound`, see [`Replica`]'s own doc comment.
    pub(crate) replica: Replica,
    /// Overrides `file_name`'s file-path-derived display name — the
    /// minimal seam a document with no `file_path` at all
    /// (and never will have one) needs to show a real name instead of the
    /// `"[No Name]"` untitled-draft fallback. `Some("Help")` for the Help
    /// virtual document; `None` for every ordinary document, where
    /// `file_name` derives its display name from `file_path` exactly as
    /// before.
    pub display_name: Option<String>,
    /// Every navigable `Ref` (link, embed, heading/block definition) in
    /// this document, in document order — rebuilt on every `view()` call
    /// from the just-synced `doc`/`buffer` pair. `navigate::
    /// follow` reads this to find what the cursor is sitting on and where a
    /// same-document or cross-document anchor lands.
    pub catalogue: Vec<rune_nav::Ref>,
    pub reading_link_focus: Option<ByteRange>,
    /// Which producer this document's content goes through —
    /// mirrored onto `doc` via `DocMachine::set_kind` every time it changes.
    /// Recomputed from `file_path` and the buffer's current content only
    /// inside `bind_path`, the single place a document acquires (or
    /// reacquires) a path; a pathless draft and the Help document therefore
    /// stay `DocumentKind::Markdown`, exactly as before this plan.
    pub kind: DocumentKind,
    pub kind_pinned: bool,
    /// The icon tier line decorations render with — mirrored
    /// onto `doc` via `DocMachine::set_icons` on every `view()` call, same
    /// pattern as `kind`/`set_kind`. `Document` holds no `App` reference,
    /// so this is a plain field an outside writer (`App::sync_view`, the
    /// same chokepoint that pushes `focused` down before every sync) sets
    /// from the one `App`-held decision (`App::icon_tier`, via `App::icons`)
    /// rather than a value this type could ever derive on its own.
    pub icons: IconSet,
    /// This document's async highlight state — spans, their
    /// version tag, and the in-flight/pending bookkeeping that bounds a
    /// document to at most one running highlight `Cmd` at a time.
    pub highlight: HighlightState,
    /// This document's graphics state — `Image` only for a
    /// `DocumentKind::Image` document (populated by `workspace::open_bytes`
    /// at open time), `Embeds` only for a `Markdown` document that has
    /// spawned at least one inline embed, `None` otherwise (including a
    /// document that used to be an image and no longer is: `kind` never
    /// changes back once bound — a document only ever acquires its `kind`
    /// once, at `bind_path`).
    graphics: crate::graphics::Graphics,
    /// The latest known [`rune_db::SyncKind`] classification for this
    /// document. Written only from authoritative ack data inside `update`
    /// dispatch — `Probe`/`Load` acks, a save-time CAS refusal, and merge's
    /// terminal outcomes (refusal, discard, clean merge, completed
    /// resolution) — never invented locally and never retimed. Consumed by
    /// chrome (the footer's disk-changed hint, tab affordances) and by
    /// merge entry's fast pre-check, which treats it strictly as a hint:
    /// the authoritative re-check is the fresh `MergePrep` landing.
    /// Nothing that mutates the buffer or the recovery store may treat it
    /// as fact. `None` before the first `Load`/`Probe` ack lands.
    pub last_sync: Option<rune_db::SyncKind>,
    /// The most recently known hard-link count for this document's file —
    /// written only from an operation's own result (`db_ack::handle_load_ack`,
    /// the launch bootstrap, a committed materialize ack's `saved`
    /// observation), never invented locally. `Some(n > 1)` is what the
    /// save-side warn in `materialize_ack::reactions::handle_materialize_ack`
    /// reads before a save forks the file from its other names on disk.
    /// `None` before any such operation has reported a fact.
    pub nlink: Option<i64>,
}

impl Document {
    /// Whether this document refuses mutation, regardless of which
    /// `ReadOnly` variant is refusing it.
    pub fn is_read_only(&self) -> bool {
        !matches!(self.read_only, ReadOnly::No)
    }

    /// Whether this document is a transient, not-yet-committed preview —
    /// `true` only for `ReadOnly::Preview`. `App::refuse_if_preview` is the
    /// one place that checks this rather than the generic `App::
    /// refuse_if_read_only`: that one also refuses `ReadOnly::Reading`,
    /// which save must NOT (^S still materializes bytes already typed in
    /// reading view) and close must NOT (closing a reading-view document is
    /// ordinary).
    pub fn is_preview(&self) -> bool {
        matches!(self.read_only, ReadOnly::Preview)
    }

    /// Whether the caret and selection background may be painted onto this
    /// document's cells: `focused && !readOnly` — an unfocused pane must
    /// not show a caret that would mislead about where keystrokes land, and
    /// a read-only document (the virtual Help tab, the error-banner
    /// document) has no insertion point to point at.
    /// `focused` itself already folds in `modal.is_none()` — see
    /// `App::sync_view`.
    pub fn has_insertion_point(&self) -> bool {
        self.focused && !self.is_read_only()
    }

    /// Whether reveal may key off the cursor — `reveal_engaged` instead of
    /// `focused`, so an open search bar (caret blurred, keystrokes owned by
    /// the bar) still reveals the element its match navigation lands the
    /// cursor in. A read-only document has nothing to reveal raw markdown
    /// under, same as it has no caret.
    pub fn reveals_under_cursor(&self) -> bool {
        self.reveal_engaged && !self.is_read_only()
    }

    /// Whether a mouse selection's background may be painted onto this
    /// document's cells. Deliberately NOT gated on `is_read_only()` the way
    /// [`Self::has_insertion_point`] is: a read-only document has no caret
    /// to place, but a mouse-drawn selection in it is real and copyable
    /// (`⌘C` never checks read-only), so hiding its highlight would leave a
    /// user action with no visible feedback. An unfocused document still
    /// shows neither overlay.
    pub fn shows_selection(&self) -> bool {
        self.focused
    }

    /// Whether `⌘R` has anything to do for this document: a whole `Image`
    /// document (`reload_image` always redecodes it) or a markdown document
    /// with at least one embed wedged mid-decode (`reload_embeds` only ever
    /// reschedules those). The dispatch gate and `reload_embeds`'s own
    /// rescheduling both read `EmbedSet::has_wedged` so neither can drift
    /// from the other — a document whose embeds are all `Live`/`Failed`
    /// answers `false` here, matching the no-op `reload_embeds` performs on
    /// it.
    pub fn has_reloadable_graphics(&self) -> bool {
        self.image().is_some()
            || self
                .embeds()
                .is_some_and(super::graphics::EmbedSet::has_wedged)
    }

    /// Whether a save is currently running for this document — derived
    /// from `save` rather than a parallel cached bool, so the two can never
    /// disagree.
    pub fn save_in_flight(&self) -> bool {
        !self.save.is_idle()
    }

    /// This document's bound recovery-store row, or `None` while
    /// `Detached`/`Binding` — the read half of [`Replica`], shared by every
    /// call site that used to read the old `db: Option<DocDb>` field
    /// directly.
    pub fn doc_db(&self) -> Option<&DocDb> {
        self.replica.doc_db()
    }

    pub fn doc_db_mut(&mut self) -> Option<&mut DocDb> {
        self.replica.doc_db_mut()
    }

    /// Whether this document's row is installed and every edit reaches the
    /// store directly — `false` for both `Detached` (no journal) and
    /// `Binding` (a journal is coming, but not installed yet).
    pub fn is_store_bound(&self) -> bool {
        self.replica.is_bound()
    }

    /// Test-only fixture setter, gated behind the same `testgrid` self-
    /// dependency trick `Cargo.toml`'s own doc comment describes: an
    /// integration test in `tests/` links this crate WITHOUT `cfg(test)`,
    /// so it can never reach `replica` (`pub(crate)`) directly the way this
    /// crate's own unit tests do. Skips `Binding` entirely — a fixture
    /// wants a document already `Bound`, never mid-round-trip.
    #[cfg(any(test, feature = "testgrid"))]
    pub fn set_doc_db_for_test(&mut self, db: DocDb) {
        self.replica = Replica::Bound(db);
    }
}

impl Document {
    pub fn new(buffer: Buffer) -> Document {
        let saved_version = buffer.version();
        let saved_content: Arc<str> = Arc::from(buffer.content());
        Document {
            buffer,
            cursors: CursorSet::new(0),
            doc: DocMachine::new(),
            viewport: Viewport::default(),
            focused: true,
            reveal_engaged: true,
            journal: Journal::new(),
            ladder_presses: 0,
            ladder_pressed_at: None,
            ladder_direction: None,
            ladder_anchor: None,
            read_only: ReadOnly::No,
            file_path: None,
            saved_version,
            saved_content,
            save: SaveState::default(),
            next_save_ticket: SaveTicketMint::default(),
            view: None,
            replica: Replica::Detached,
            display_name: None,
            catalogue: Vec::new(),
            reading_link_focus: None,
            kind: DocumentKind::Markdown,
            kind_pinned: false,
            icons: IconSet::unicode(),
            highlight: HighlightState::default(),
            graphics: crate::graphics::Graphics::None,
            last_sync: None,
            nlink: None,
            pinned: false,
        }
    }

    /// The one dirtiness derivation, read by every consumer (render, the
    /// close/quit guards, save gating, trash gating alike): the live
    /// buffer's version against `saved_version` first, as a cheap
    /// short-circuit — the overwhelmingly common case is a clean document,
    /// where the version comparison alone already answers `false` without
    /// ever touching the bytes. Only when the version moved does the byte
    /// comparison against `saved_content` run, which is what makes an
    /// edit-then-undo document read clean again: `Buffer::apply_edits`
    /// always returns `version + 1`, and undo/redo build a new buffer, so a
    /// version comparison alone would leave such a document dirty forever
    /// even though the bytes are back to identical.
    pub fn is_dirty(&self) -> bool {
        self.buffer.version() != self.saved_version && self.buffer.content() != &*self.saved_content
    }

    /// Arms the no-store fallback save in flight, capturing `version`/
    /// `content` TOGETHER at one chokepoint — the only way `save` ever
    /// leaves `Idle` for a `Direct` attempt, so a save can never be in
    /// flight without the exact bytes it captured. Called by every no-store
    /// save-start site: the fallback in `save::trigger_save`, and
    /// `save::materialize_now`'s own no-binding/enqueue-failure fallbacks.
    pub fn begin_save(&mut self, version: u64, content: Arc<str>) -> SaveTicket {
        let ticket = self.next_save_ticket.mint();
        self.save.begin_direct(ticket, version, content);
        ticket
    }

    /// The current save phase — `on_store_failure`'s own state-aware sweep
    /// is the primary reader: `Preparing` and an unpublished `Recording`
    /// abandon on a store failure, `Publishing` and `Direct` do not (the
    /// write is already headed to, or already on, disk and the store's
    /// death cannot cancel it), and a published `Recording` resolves as a
    /// synthetic commit.
    pub fn save_phase(&self) -> SavePhase {
        self.save.phase()
    }

    /// Arms the store-backed materialize dance's `Preparing` phase — the
    /// counterpart to `begin_save` for a save with a `MaterializePrepare`
    /// enqueued. `save::materialize_now`/`save::bind_new_now` are the only
    /// callers.
    pub(crate) fn begin_prepare(
        &mut self,
        version: u64,
        content: Arc<str>,
        params: PublishParams,
        prep_op: u64,
    ) -> SaveTicket {
        let ticket = self.next_save_ticket.mint();
        self.save
            .begin_prepare(ticket, version, content, params, prep_op);
        ticket
    }

    /// The `MaterializePrepare` op id `Preparing` is waiting on, or `None`
    /// outside that state — `db_dispatch`'s own op-id routing already
    /// filters by this before `materialize_ack::handle_prepare_ack` is
    /// ever reached, so this is a defense-in-depth re-check, not the only
    /// gate.
    pub(crate) fn prep_op(&self) -> Option<u64> {
        self.save.prep_op()
    }

    pub(crate) fn preparing_mode(&self) -> Option<crate::save::SaveMode> {
        self.save.preparing_mode()
    }

    /// This document's current save attempt ticket, or `None` when `Idle` —
    /// the correlation key every ticketed `Msg` echoes back so a reply for
    /// an attempt this document has already moved on from is a typed,
    /// silent drop rather than a promotion against the wrong capture.
    pub fn save_ticket(&self) -> Option<SaveTicket> {
        self.save.ticket()
    }

    /// Advances `Preparing` to `Publishing` once its prep ack lands and the
    /// caller-side vfs `Cmd` is about to be spawned — `false` (a no-op) for
    /// a stale/late call against a document that already moved on. The
    /// store dying while `Publishing` no longer releases this document
    /// (`on_store_failure`'s own state-aware handling) — the vfs `Cmd` this
    /// transition is about to spawn owns the save until it replies, exactly
    /// once, and no second attempt can start while it does (`save_in_
    /// flight()` stays `true` the whole time).
    pub(crate) fn begin_publishing(&mut self) -> Option<(SaveTicket, Arc<str>, PublishParams)> {
        self.save
            .advance_to_publishing()
            .map(|(ticket, capture, params)| (ticket, capture.content, params))
    }

    /// Whether `save` is currently `Publishing` — the invariant chokepoint:
    /// while this is `true`, `on_store_failure` must never abandon this
    /// document, and no second save attempt can be spawned for it (`save_
    /// in_flight()` already refuses one).
    pub(crate) fn is_publishing(&self) -> bool {
        self.save.is_publishing()
    }

    /// Advances `Publishing` to `Recording` once the vfs `Cmd`'s outcome is
    /// known and a `MaterializeRecord` op has been enqueued — `false` (a
    /// no-op) for a stale/late call against a document that already moved
    /// on. `published` marks whether the disk write already physically
    /// took effect (`Committed`/`Raced`) — `save_phase`'s own `Recording {
    /// published }` is what `on_store_failure`'s state-aware handling reads
    /// back to decide whether a lost record ack still resolves as a
    /// synthetic commit.
    pub(crate) fn begin_recording(&mut self, record_op: u64, published: bool) -> bool {
        self.save.advance_to_recording(record_op, published)
    }

    /// The `MaterializeRecord` op id `Recording` is waiting on, or `None`
    /// outside that state.
    pub fn record_op(&self) -> Option<u64> {
        self.save.record_op()
    }

    /// The path an in-flight `bind_new` CREATE (`save::bind_new_now`) is
    /// trying to claim, or `None` outside that shape — deliberately not
    /// `file_path`: a create that loses the no-clobber race must leave the
    /// draft untitled, or a later ^S would overwrite the winner.
    pub fn bind_target(&self) -> Option<&PathBuf> {
        self.save.bind_target()
    }

    /// Takes `bind_target`, leaving `None` behind — `handle_materialize_
    /// ack`'s committed arm moves it into `file_path` only once the write
    /// actually commits; every refusal arm clears it without binding.
    pub(crate) fn take_bind_target(&mut self) -> Option<PathBuf> {
        self.save.take_bind_target()
    }

    /// Resolves a successful save ack: returns `save` to `Idle`, and
    /// promotes the captured content into `saved_content` iff its version
    /// matches `version` (the ack's own correlated fact) AND exceeds
    /// `saved_version` — so an ack for a capture this document no longer
    /// recognizes (or one that would move the baseline BACKWARD) promotes
    /// nothing. Returns whether the promotion happened. The only writer of
    /// `saved_content`/`saved_version`.
    pub fn finish_save_ok(&mut self, version: u64) -> bool {
        let Some(capture) = self.save.resolve() else {
            return false;
        };
        if capture.version == version && capture.version > self.saved_version {
            self.saved_version = capture.version;
            self.saved_content = capture.content;
            true
        } else {
            false
        }
    }

    /// Resolves a failed/abandoned save: returns `save` to `Idle` and drops
    /// the captured bytes without promoting anything — the exact opposite
    /// of `finish_save_ok`. Every clear site that isn't a genuine success
    /// (`fail_materialize_locally`, `on_store_failure`'s sweep,
    /// `bind_new_now`'s error arm, `handle_save_done`'s `Err` arm) goes
    /// through here, never through a direct field write.
    pub fn abandon_save(&mut self) {
        self.save.resolve();
    }

    /// The version an in-flight save captured, if one is running — a
    /// read-only peek (never mutates) for `materialize_ack::
    /// handle_materialize_ack`'s ack-side chokepoint, which needs it to
    /// correlate `MatResult` (which carries no version of its own) against
    /// the SAME bytes `begin_save` captured before deciding whether to call
    /// `finish_save_ok` or `abandon_save`.
    pub fn pending_save_version(&self) -> Option<u64> {
        self.save.pending_version()
    }

    /// The one hydration-adoption chokepoint: `self.buffer` is
    /// assumed to hold exactly `disk_content` (the caller's job — the
    /// bootstrap path just loaded it straight off disk; `db::handle_load_ack`
    /// checks the buffer's version hasn't moved since `Load` was issued
    /// first). Applies three things every hydration route must do
    /// identically, so they cannot drift apart again:
    ///
    /// (a) the destructive-async-reset suspicion check — refuses an
    /// adoption that would empty or drastically shrink a non-empty buffer,
    /// leaving `self` untouched;
    /// (b) journals the adoption as one synthetic bridge `Step` (pushed
    /// directly, never through `commands::edit::commit_edit_batch`/`db::
    /// append_edit` — the durable side already has this content, only the
    /// LOCAL undo journal needs the anchor) so ⌘Z reaches `disk_content`;
    /// (c) surfaces an `apply_edits` failure as a refusal rather than an
    /// unannotated no-op. Dirtiness is never marked here: an adopting
    /// hydration leaves the buffer genuinely different from
    /// `saved_content`, so `is_dirty` already reports it dirty on its very
    /// next read, with no separate step required.
    ///
    /// Deliberately NOT gated on `read_only`, including
    /// `ReadOnly::Reading`. Hydration adopts the user's OWN unsaved
    /// recovered draft — refusing it to honour a view mode would DISCARD
    /// already-typed bytes, a worse failure than a buffer changing once
    /// under a reading view (the prime directive that the user's words
    /// win). This is silent by design nowhere else: state it
    /// here so it reads as a decision, not an oversight.
    pub fn hydrate(&mut self, disk_content: &str, recovered: &str) -> Hydration {
        if recovered == disk_content {
            return Hydration::NoChange;
        }
        if is_suspicious_shrink(disk_content, recovered) {
            return Hydration::Refused(
                "recovered draft looked truncated relative to the file on disk — kept the on-disk version",
            );
        }
        let edit = SortedEdits::single(Edit {
            start: 0,
            end: self.buffer.len(),
            insert: recovered.to_string(),
        });
        let Ok((new_buffer, applied)) = self.buffer.apply_edits(&edit) else {
            return Hydration::Refused("recovered draft failed to apply to the buffer");
        };
        self.cursors = self.cursors.map(|c| {
            let position = recovered.floor_char_boundary(c.position.min(recovered.len()));
            let anchor = recovered.floor_char_boundary(c.anchor.min(recovered.len()));
            Cursor {
                position,
                anchor,
                desired_col: if position == c.position {
                    c.desired_col
                } else {
                    0
                },
                ..c
            }
        });
        self.buffer = new_buffer;
        self.journal.push(Step {
            edits: applied,
            cursors_before: Vec::new(),
            cursors_after: Vec::new(),
            kind: EditKind::Other,
        });
        Hydration::Adopted
    }

    pub fn file_name(&self) -> &str {
        if let Some(name) = self.display_name.as_deref() {
            return name;
        }
        self.file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("[No Name]")
    }

    /// The only way a document acquires (or reacquires) a path. Clears any
    /// `display_name` override so `file_name()` derives from the new path
    /// (one value, one meaning) — a document once shown under a
    /// placeholder name (an "Untitled N" draft, a rename in progress) must
    /// switch over to its real name the moment it actually has one. Also
    /// the only place `kind` is recomputed — pushed into
    /// `doc` too, so `DocMachine::sync_content` picks the right producer on
    /// its very next call.
    pub fn bind_path(&mut self, path: PathBuf) {
        if !self.kind_pinned {
            let kind = kind_for(Some(&path), self.buffer.content());
            self.kind = kind;
            self.doc.set_kind(self.kind);
        }
        self.file_path = Some(path);
        self.display_name = None;
    }
}
