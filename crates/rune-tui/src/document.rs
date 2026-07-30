//! `DocumentId` + `Document`: one open editing pane's full state — buffer,
//! cursors, the display-pipeline root machine, the scrollable viewport onto
//! it, file identity, save/dirty bookkeeping, and its own recovery-store
//! handle (plan WP1 decision 2: "Fat Document, no View split" — `Document`
//! absorbs everything the pre-WP1 `Editor` held plus every per-doc field
//! that used to live directly on `App`). `Document::sync` is the fixed
//! per-message sync sequence (plan Context, "Msg/Cmd runtime": `sync_content`
//! iff version changed -> `set_width` -> `sync_cursors` -> `snapshot` ->
//! scroll-to-cursor -> re-`view` -> scroll-to-cursor again -> re-`view` once
//! more, since a scroll command can move the cursor itself, and that move
//! can itself change reveal-driven display geometry the first reconcile
//! already settled against — see `sync`'s own docs).

use std::num::NonZeroU64;
use std::path::PathBuf;

use rune_core::buffer::{Buffer, Edit};
use rune_core::coords::WrapPoint;
use rune_core::cursor::{Cursor, CursorSet};
use rune_core::undo::{Journal, Step};
use rune_md::element::doc::{DocMachine, ViewSnapshots};
use rune_syntax::DocumentKind;

use crate::db::DocDb;
use crate::document_support::{is_suspicious_shrink, kind_for};
pub use crate::document_support::Hydration;
use crate::highlight::HighlightState;
use crate::viewport::Viewport;

/// Identifies one open `Document` for the lifetime of the process — minted
/// monotonically by `App::next_doc_id` (plan WP1 decision 1). Tabs and every
/// doc-scoped `Msg` key on this, never on a path: help/untitled documents
/// are first-class and have no path at all. The inner `NonZeroU64` is
/// `pub(crate)`, not private: `App` — the sole minter, via
/// `App::mint_doc_id` — constructs one directly from its own `NonZeroU64`
/// counter, with no fallible conversion step to route around.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DocumentId(pub(crate) NonZeroU64);

/// One open editing pane's complete state (plan WP1 decision 2): buffer,
/// cursors, the root display machine, the viewport onto it, file identity,
/// save/dirty bookkeeping, and this doc's own recovery-store handle.
/// `pending_quit`/`status_message`/`db_banner`/`should_quit` stay on `App`
/// (app-wide, not per-document); `pending_save_confirm` also stays on `App`
/// but is doc-tagged (`Option<(DocumentId, u32)>`) so a tab switch can't
/// misapply an armed confirm gate.
pub struct Document {
    pub buffer: Buffer,
    pub cursors: CursorSet,
    pub doc: DocMachine,
    pub viewport: Viewport,
    pub focused: bool,
    /// The in-memory undo/redo journal (WP7): every applied edit batch is
    /// pushed here as a `Step`; `commands::edit::undo`/`redo` peek-then-
    /// commit against it (plan Context, "Undo journal").
    pub journal: Journal,
    /// Guards every buffer-mutating command (typing, backspace/delete,
    /// indent/outdent, cut, paste — anything that reaches
    /// `commands::edit::commit_edit_batch`, the sole writer of buffer
    /// mutations) against touching a read-only document. Checked at that ONE
    /// chokepoint rather than at each command's call site, so no future
    /// mutating command can forget the guard (review finding F1: an earlier
    /// version checked this only in `commands::clipboard::handle_paste_
    /// content`, leaving Cut and every keyboard-insert path able to mutate a
    /// "read-only" document — the exact Go bug `commands_clipboard.go`'s
    /// comment describes, reintroduced by guarding the wrong layer).
    /// `commands::edit::undo`/`redo` deliberately do NOT check this field —
    /// Go's own `ApplyInverse`/`Reapply` (`edit_primitives.go`) bypass
    /// `m.readOnly` the same way, unlike `ReplaceRange`
    /// (`edit_primitives.go`) which checks it first.
    pub read_only: bool,
    /// The file this document is bound to, or `None` for an untitled draft
    /// (moved off `App` in WP1: every open document has its own identity).
    pub file_path: Option<PathBuf>,
    /// The buffer version the LAST successful save/materialize ack
    /// persisted — advanced ONLY from a store ack (`save::handle_materialize_
    /// ack`) or, for the no-store fallback path, `Msg::SaveDone` (see
    /// `save::trigger_save`'s docs). Never read directly by `is_dirty` — see
    /// `is_dirty_cached`.
    pub saved_version: u64,
    /// The version `materialize`/the fallback save `Cmd` targets while a
    /// save is in flight — carried so its eventual ack only ever advances
    /// `saved_version` to the version IT captured, never the buffer's
    /// current (possibly further-edited) version.
    pub save_pending_version: Option<u64>,
    pub save_in_flight: bool,
    /// The path an in-flight `bind_new` materialize is trying to CREATE
    /// (`save::bind_new_now`). Deliberately not `file_path`: a create that
    /// loses the no-clobber race must leave the draft untitled, or a later
    /// ⌘S would overwrite the winner (§0.1 rung 1). `handle_materialize_ack`
    /// moves it into `file_path` only once the write actually commits.
    pub pending_bind_path: Option<PathBuf>,
    /// The render-only dirty cache (CONSTITUTION §1.4.8): `is_dirty` reads
    /// ONLY this field. Recomputed in `update`, and ONLY there, at exactly
    /// two trigger points — see `materialize_ack::recompute_dirty`'s doc comment.
    /// `pub(crate)` (not private) because the recompute chokepoint now lives
    /// in a different module (`save.rs`).
    pub(crate) is_dirty_cached: bool,
    /// The most recent display-pipeline snapshot, cached by `App::sync_view`
    /// for `render::draw` to blit. `None` only before this document's first
    /// sync.
    pub view: Option<ViewSnapshots>,
    /// This document's handle onto the app-wide recovery store (plan WP1
    /// decision 5: `AppDb` split into app-level `Db` and per-doc `DocDb`).
    /// `None` for a document with no recovery journal — an ephemeral/help
    /// document, or one opened before per-doc hydration exists (Assumption
    /// A1).
    pub db: Option<DocDb>,
    /// Overrides `file_name`'s file-path-derived display name (plan
    /// WP7.S2) — the minimal seam a document with no `file_path` at all
    /// (and never will have one) needs to show a real name instead of the
    /// `"[No Name]"` untitled-draft fallback. `Some("Help")` for the Help
    /// virtual document; `None` for every ordinary document, where
    /// `file_name` derives its display name from `file_path` exactly as
    /// before.
    pub display_name: Option<String>,
    /// Every navigable `Ref` (link, embed, heading/block definition) in
    /// this document, in document order — rebuilt on every `view()` call
    /// from the just-synced `doc`/`buffer` pair (plan WP5.S2). `navigate::
    /// follow` reads this to find what the cursor is sitting on and where a
    /// same-document or cross-document anchor lands.
    pub catalogue: Vec<rune_nav::Ref>,
    /// Which producer this document's content goes through (plan WP4) —
    /// mirrored onto `doc` via `DocMachine::set_kind` every time it changes.
    /// Recomputed from `file_path` only inside `bind_path`, the single place
    /// a document acquires (or reacquires) a path; a pathless draft and the
    /// Help document therefore stay `DocumentKind::Markdown`, exactly as
    /// before this plan.
    pub kind: DocumentKind,
    /// This document's async highlight state (plan WP5) — spans, their
    /// version tag, and the in-flight/pending bookkeeping that bounds a
    /// document to at most one running highlight `Cmd` at a time.
    pub highlight: HighlightState,
}

impl Document {
    /// Whether the caret and selection background may be painted onto this
    /// document's cells. Go's three overlay gates (`textedit/render.go`) are
    /// all `focused && !readOnly`: an unfocused pane must not show a caret
    /// that would mislead about where keystrokes land, and a read-only
    /// document (the virtual Help tab, the error-banner document) has no
    /// insertion point to point at. `focused` itself already folds in
    /// `modal.is_none()` — see `App::sync_view`.
    pub fn shows_caret(&self) -> bool {
        self.focused && !self.read_only
    }
}

impl Document {
    pub fn new(buffer: Buffer) -> Document {
        let saved_version = buffer.version();
        Document {
            buffer,
            cursors: CursorSet::new(0),
            doc: DocMachine::new(),
            viewport: Viewport::default(),
            focused: true,
            journal: Journal::new(),
            read_only: false,
            file_path: None,
            saved_version,
            save_pending_version: None,
            save_in_flight: false,
            pending_bind_path: None,
            is_dirty_cached: false,
            view: None,
            db: None,
            display_name: None,
            catalogue: Vec::new(),
            kind: DocumentKind::Markdown,
            highlight: HighlightState::default(),
        }
    }

    /// Reads the render-only dirty cache — see `materialize_ack::recompute_dirty`'s
    /// doc comment for the two points that keep it current.
    pub fn is_dirty(&self) -> bool {
        self.is_dirty_cached
    }

    /// Marks the buffer dirty relative to the file it was hydrated from.
    /// Called from [`Document::hydrate`] on every adoption — both the
    /// bootstrap path (`rune-cli::main`, before the runtime loop) and the
    /// live per-document hydration ack (`db::handle_load_ack`, from inside
    /// `update`) reach this through that one chokepoint, never directly.
    pub fn mark_dirty_from_hydration(&mut self) {
        self.is_dirty_cached = true;
    }

    /// The one hydration-adoption chokepoint (plan WP5.S2): `self.buffer` is
    /// assumed to hold exactly `disk_content` (the caller's job — the
    /// bootstrap path just loaded it straight off disk; `db::handle_load_ack`
    /// checks the buffer's version hasn't moved since `Load` was issued
    /// first). Applies three things every hydration route must do
    /// identically, so they cannot drift apart again:
    ///
    /// (a) the §1.3 destructive-async-reset suspicion check — refuses an
    /// adoption that would empty or drastically shrink a non-empty buffer,
    /// leaving `self` untouched;
    /// (b) journals the adoption as one synthetic bridge `Step` (pushed
    /// directly, never through `commands::edit::commit_edit_batch`/`db::
    /// append_edit` — the durable side already has this content, only the
    /// LOCAL undo journal needs the anchor) so ⌘Z reaches `disk_content`;
    /// (c) surfaces an `apply_edits` failure as a refusal rather than an
    /// unannotated no-op.
    pub fn hydrate(&mut self, disk_content: &str, recovered: &str) -> Hydration {
        if recovered == disk_content {
            return Hydration::NoChange;
        }
        if is_suspicious_shrink(disk_content, recovered) {
            return Hydration::Refused(
                "recovered draft looked truncated relative to the file on disk — kept the on-disk version",
            );
        }
        let edit = Edit {
            start: 0,
            end: self.buffer.len(),
            insert: recovered.to_string(),
        };
        let Ok((new_buffer, applied)) = self.buffer.apply_edits(std::slice::from_ref(&edit)) else {
            return Hydration::Refused("recovered draft failed to apply to the buffer");
        };
        self.cursors = self.cursors.adjust_after_batch_edits(&applied);
        self.buffer = new_buffer;
        self.journal.push(Step {
            edits: applied,
            cursors_before: Vec::new(),
            cursors_after: Vec::new(),
        });
        self.mark_dirty_from_hydration();
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
    /// (§1.7: one value, one meaning) — a document once shown under a
    /// placeholder name (an "Untitled N" draft, a rename in progress) must
    /// switch over to its real name the moment it actually has one. Also
    /// the only place `kind` is recomputed (plan WP4.S4) — pushed into
    /// `doc` too, so `DocMachine::sync_content` picks the right producer on
    /// its very next call.
    pub fn bind_path(&mut self, path: PathBuf) {
        self.kind = kind_for(Some(&path));
        self.doc.set_kind(self.kind);
        self.file_path = Some(path);
        self.display_name = None;
    }

    /// The pure QUERY half of the per-message sync sequence (plan Context,
    /// "Msg/Cmd runtime"): `sync_content` iff version changed -> `set_width`
    /// -> `sync_cursors` -> `snapshot`. Deliberately does NOT touch
    /// `viewport.scroll_row` — see `scroll_to_cursor`'s docs (review finding
    /// F4: separating the snapshot-returning query from the scroll
    /// mutation removes the double-write/double-computation `sync` used to
    /// cause).
    ///
    /// Idempotent/cheap when nothing changed — `sync_content`/
    /// `sync_cursors` are no-ops in that case (plan Gotchas: "Reveal must
    /// never bump the buffer version") — so `commands::nav`/`commands::edit`
    /// call this freely, more than once per message batch, to get
    /// Buffer<->Syntax<->Wrap coordinate conversions that reflect the
    /// CURRENT `Document` fields (in particular a `Resize` already applied
    /// earlier in the same batch — see their module docs) before computing
    /// a new cursor position.
    pub fn view(&mut self) -> ViewSnapshots {
        self.doc.set_focus(self.focused);
        self.sync_catalogue();
        self.doc.set_width(self.viewport.width);
        self.doc.sync_cursors(&self.buffer, &self.cursors);
        self.doc.snapshot(&self.buffer)
    }

    /// The narrower, WIDTH-FREE half of `view()`'s parse step (plan WP5.S6,
    /// [rune-tui A 14]): re-syncs the comrak parse and rebuilds `catalogue`
    /// from it, without `view()`'s width-dependent wrap pass or cursor/
    /// snapshot work. `navigate::land_anchor` needs a just-opened target
    /// document's catalogue to find an anchor's heading BEFORE that
    /// document is necessarily ever on screen (no viewport width to wrap
    /// against yet) — exposed here, the one chokepoint `view()` itself
    /// calls into, rather than re-inlining these same two lines at that
    /// call site where they'd be free to drift from this sequence.
    pub fn sync_catalogue(&mut self) {
        let built_before = self.doc.built_version();
        self.doc.sync_content(&self.buffer);
        // The catalogue is derived solely from buffer content + blocks, so
        // it only needs rebuilding when `sync_content` actually reparsed (a
        // real content edit, or the very first call) — not on every call,
        // which commands may make several times per message batch.
        if self.doc.built_version() != built_before {
            self.catalogue =
                rune_md::catalogue::catalogue(self.buffer.content(), self.doc.blocks());
        }
    }

    /// Scrolls the viewport so the PRIMARY cursor's current row is visible.
    /// The single writer of `viewport.scroll_row` (review finding F4: "no
    /// shadow state" — a value has exactly one writer). Callers that only
    /// need coordinate conversions (`commands::nav`/`commands::edit`) must
    /// use `view()` instead and never call this themselves: calling it
    /// mid-motion would scroll toward a cursor position that's about to
    /// change again later in the same batch, then get silently overwritten
    /// by the batch's real settle — wasted work at best, a visibly wrong
    /// intermediate scroll at worst.
    ///
    /// `viewport.scroll_row` is a DISPLAY row (WP3: what `render::build_rows`
    /// actually indexes, table borders included), but the cursor's own row
    /// is always WRAP space (border rows aren't addressable by the caret) —
    /// `view.display.wrap_to_display` converts before `reconcile` ever sees
    /// it, and the row `reconcile` hands back (also display-space) converts
    /// the OTHER way, through `display_to_wrap`, before `snap_cursor_to_row`
    /// (which computes a wrap-space cursor position) ever sees it. Missing
    /// either conversion scrolls every document containing a table wrong by
    /// the number of border rows above the cursor.
    pub fn scroll_to_cursor(&mut self, view: &ViewSnapshots) {
        let primary = self.cursors.primary();
        let buffer_point = self.buffer.offset_to_line_col(primary.position);
        let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
        let display_row = view.display.wrap_to_display(wrap_point.row);
        if let Some(target_row) = self
            .viewport
            .reconcile(display_row, view.display.total_rows())
        {
            let wrap_row = view.display.display_to_wrap(target_row);
            self.snap_cursor_to_row(view, wrap_row);
        }
    }

    /// The `Viewport::reconcile` `Independent`-mode counterpart: a
    /// `commands::nav_scroll` command already moved the viewport on its own
    /// and left the PRIMARY cursor outside the scrolloff-padded band, so it
    /// snaps onto `row` at that cursor's own `desired_col` (the same visual-
    /// column-preserving convention `commands::nav::move_row` uses) —
    /// collapsing any selection and any secondary cursor, exactly like
    /// `commands::nav::escape`'s multi-cursor collapse (plan WP7.S1: "the
    /// cursor is moved onto the window").
    fn snap_cursor_to_row(&mut self, view: &ViewSnapshots, row: usize) {
        let primary = self.cursors.primary();
        let col = view
            .wrap
            .byte_col_from_visual(self.buffer.content(), row, primary.desired_col);
        let syntax_point = view
            .wrap
            .wrap_to_syntax(self.buffer.content(), WrapPoint { row, col });
        let buffer_point = view.syntax.syntax_to_buffer(syntax_point);
        let offset = self.buffer.line_col_to_offset(buffer_point);
        let snapped = Cursor {
            position: offset,
            anchor: offset,
            desired_col: primary.desired_col,
            id: primary.id,
        };
        self.cursors = self.cursors.collapse_to(snapped);
    }

    /// The fixed per-BATCH settle sequence: rebuild the view, scroll to the
    /// (by now final) cursor, then reconcile the viewport a SECOND time
    /// against whatever that scroll itself settled. `App::sync_view` —
    /// called once per whole message batch by the runtime (`runtime::run`)
    /// and by tests that need the settled state — is the only caller;
    /// movement/editing commands call `view()` alone (see its docs).
    ///
    /// Re-views once more AFTER `scroll_to_cursor`, not before: reveal state
    /// is a function of `self.cursors` (`RevealGrant::Decide`'s cursor-probe
    /// policies) — a boxed table's OWN reveal state is exactly such a
    /// policy (`emit_table`: a table is rendered as its full bordered Grid/
    /// Wrapped/Pivoted layout, or (cursor inside it) as bare verbatim source
    /// lines, one whole-table `RevealSm` decision) — and `scroll_to_cursor`
    /// can itself move `self.cursors`: a `commands::nav_scroll` command's
    /// `Independent`-mode scroll leaves the viewport where it put it and
    /// instead snaps the cursor onto the now-settled window (`Viewport::
    /// reconcile`'s docs). The first `view()` above necessarily samples
    /// reveal against the PRE-scroll cursor — `scroll_to_cursor` needs that
    /// view's coordinate maps to decide where the cursor should land in the
    /// first place, so the two can't run in the other order.
    ///
    /// That first `scroll_to_cursor` call reconciles the viewport against
    /// THAT pre-final view's row geometry (`total_rows`, the wrap<->display
    /// maps) — geometry a table's own reveal transition can change out from
    /// under it: snapping the cursor INTO a boxed table collapses it from
    /// its bordered layout to bare source lines (fewer rows entirely), so
    /// the `scroll_row`/band `reconcile` just computed can land outside the
    /// scrolloff band the MOMENT the settled, reveal-updated `total_rows`
    /// replaces the stale one it was computed against — the exact "8 rows
    /// before, 9 after" `SYNC-IDEMPOTENT` shape, caught only by a later,
    /// message-free `App::sync_view` first discovering the un-reconciled
    /// mismatch. Re-viewing without reconciling again would still hand back
    /// (and let `DocMachine` cache) a snapshot whose viewport was settled
    /// against stale geometry.
    ///
    /// The second `scroll_to_cursor` call closes that gap: `mode` was
    /// already consumed back to `FollowCursor` by the first call, so this
    /// pass only ever ADJUSTS `scroll_row` to keep the (unchanged, already-
    /// settled) cursor row inside the band of the NOW-final geometry — it
    /// never moves the cursor again (`Viewport::reconcile`'s `FollowCursor`
    /// arm), so reveal cannot re-trigger from this second pass, and the
    /// final `view()` is guaranteed a `DocMachine::snapshot` memo hit. Both
    /// extra passes are free whenever nothing this batch actually triggered
    /// a reveal-driven geometry change (the common case: no scroll command,
    /// or a scroll command whose target lies outside any reveal-sensitive
    /// element).
    pub fn sync(&mut self) -> ViewSnapshots {
        let view = self.view();
        self.scroll_to_cursor(&view);
        let settled = self.view();
        self.scroll_to_cursor(&settled);
        self.view()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_vfs::Mem;

    #[test]
    fn sync_reparses_once_and_is_idempotent_on_repeat_calls() {
        let mut doc = Document::new(Buffer::new("# hello\nworld\n"));
        doc.viewport.set_size(80, 24);
        let first = doc.sync();
        // "# hello" + "world" + the trailing empty line from the final \n.
        assert_eq!(first.display.total_rows(), 3);
        let second = doc.sync();
        assert_eq!(second.display.total_rows(), first.display.total_rows());
    }

    /// The `TODO-fuzz-sync-idempotent-table-scroll.md` regression, pinned
    /// directly against `Document::sync` rather than only through the
    /// checked-in fuzz replay (`crates/rune-fuzz/repros/sync-idempotent-
    /// 04.rune`): `scroll_line_down` (Independent-mode `ctrl+down`) snaps
    /// the cursor INTO a boxed table, which is itself a `RevealGrant::
    /// Decide` policy (`rune_md::emit::table::emit_table`) — collapsing the
    /// table from its bordered layout to bare source lines shrinks
    /// `total_rows` out from under the `Viewport::reconcile` call that just
    /// ran against the PRE-collapse geometry, leaving `scroll_row` outside
    /// the settled scrolloff band. A second, message-free `sync()` must not
    /// see this catch up on its own — `sync()` itself must already be a
    /// fixpoint.
    #[test]
    fn sync_reconciles_the_viewport_again_after_a_reveal_driven_geometry_shrink() {
        let content = "# Doc\n\n| Name | Age |\n| :--- | ---: |\n\
                        | Alice | 30 |\n| Bob | 25 |\n\ntail\n";
        let mut doc = Document::new(Buffer::new(content));
        doc.viewport.set_size(80, 24);
        doc.focused = true;

        crate::commands::nav_scroll::scroll_line_down(&mut doc);
        let first = doc.sync();
        let scroll_after_first_sync = doc.viewport.scroll_row;

        let second = doc.sync();
        assert_eq!(
            second.display.total_rows(),
            first.display.total_rows(),
            "a second, message-free sync() changed the rendered row count"
        );
        assert_eq!(
            doc.viewport.scroll_row, scroll_after_first_sync,
            "a second, message-free sync() moved scroll_row"
        );
    }

    #[test]
    fn document_ids_are_distinct_and_ordered() {
        // Mints two REAL ids the same way production code does — through
        // `App`, never a raw-number constructor.
        let mut app = crate::app::App::new(
            Buffer::new("a"),
            None,
            std::sync::Arc::new(Mem::new()),
            None,
        );
        let a = app.active;
        let b = app.open_document(Buffer::new("b"));
        assert_ne!(a, b);
        assert!(a < b);
    }
}
