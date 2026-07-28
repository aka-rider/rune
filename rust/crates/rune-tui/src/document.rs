//! `DocumentId` + `Document`: one open editing pane's full state — buffer,
//! cursors, the display-pipeline root machine, the scrollable viewport onto
//! it, file identity, save/dirty bookkeeping, and its own recovery-store
//! handle (plan WP1 decision 2: "Fat Document, no View split" — `Document`
//! absorbs everything the pre-WP1 `Editor` held plus every per-doc field
//! that used to live directly on `App`). `Document::sync` is the fixed
//! per-message sync sequence (plan Context, "Msg/Cmd runtime": `sync_content`
//! iff version changed -> `set_width` -> `sync_cursors` -> `snapshot` ->
//! scroll-to-cursor).

use std::num::NonZeroU64;
use std::path::PathBuf;

use rune_core::buffer::Buffer;
use rune_core::coords::WrapPoint;
use rune_core::cursor::{Cursor, CursorSet};
use rune_core::undo::Journal;
use rune_md::element::doc::{DocMachine, ViewSnapshots};

use crate::db::DocDb;

/// The vim/Helix scrolloff default (Helix's own default), clamped per
/// viewport at `reconcile` time (plan WP7.S1) so a tiny pane still has a
/// valid `[top, bottom]` band.
const DEFAULT_SCROLLOFF: u16 = 5;

/// Which side drives the next `Viewport::reconcile` call (plan WP7.S1):
/// `FollowCursor` — every ordinary motion command, and the default — means
/// the CURSOR moved and the viewport must chase it, honouring `scrolloff`.
/// `Independent` means a `commands::nav_scroll` scroll command already moved
/// `scroll_row` on its own (vim `scroll.txt`'s "the cursor is moved onto the
/// window" case; Helix `commands::scroll(..., sync_cursor: false)`) — the
/// viewport stays exactly where that command put it, and `reconcile` snaps
/// the CURSOR back into view instead if it fell outside the padded band.
/// `reconcile` always resets this to `FollowCursor` once consumed, so
/// exactly one `Independent` reconciliation is ever spent per scroll
/// command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollMode {
    #[default]
    FollowCursor,
    Independent,
}

/// Identifies one open `Document` for the lifetime of the process — minted
/// monotonically by `App::next_doc_id` (plan WP1 decision 1). Tabs and every
/// doc-scoped `Msg` key on this, never on a path: help/untitled documents
/// are first-class and have no path at all. The inner `NonZeroU64` is
/// `pub(crate)`, not private: `App` — the sole minter, via
/// `App::mint_doc_id` — constructs one directly from its own `NonZeroU64`
/// counter, with no fallible conversion step to route around.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DocumentId(pub(crate) NonZeroU64);

/// The visible window onto the wrapped document: `width`/`height` in cells,
/// `scroll_row` the first visible wrap row (plan Context, "Cell model" /
/// coords.rs `WrapRow`).
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
    pub scroll_row: usize,
    /// The minimum number of wrap rows kept visible above/below the cursor
    /// (plan WP7.S1) — Helix's default (`DEFAULT_SCROLLOFF`), clamped at
    /// `reconcile` time to at most half the viewport height.
    pub scrolloff: u16,
    /// Which side is authoritative for the NEXT `reconcile` call — see
    /// `ScrollMode`'s docs. Reset to `FollowCursor` by `reconcile` itself
    /// once consumed.
    pub mode: ScrollMode,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            width: 80,
            height: 24,
            scroll_row: 0,
            scrolloff: DEFAULT_SCROLLOFF,
            mode: ScrollMode::FollowCursor,
        }
    }
}

impl Viewport {
    pub fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    /// `scrolloff`, clamped so `[scroll_row + off, scroll_row + height - 1
    /// - off]` is never empty — `(height - 1) / 2` is the largest `off` for
    /// which `off <= height - 1 - off` still holds (plan WP7.S1: "clamped
    /// to half the viewport height so it degrades in a tiny pane"). A
    /// larger clamp (plain `height / 2`) would let the two bounds cross on
    /// an even-height viewport, breaking the one-step convergence
    /// `SYNC-IDEMPOTENT` (`rune-fuzz/src/invariant/render.rs`) requires.
    fn effective_scrolloff(&self) -> usize {
        let height = self.height as usize;
        (self.scrolloff as usize).min(height.saturating_sub(1) / 2)
    }

    /// The vim/Helix scrolloff invariant (plan WP7.S1, module docs): the
    /// cursor is never left outside the viewport. Replaces the old
    /// `scroll_to_row` (Go/vim parity note: "If the cursor position is
    /// moved off of the window, the cursor is moved onto the window (with
    /// 'scrolloff' screen lines around it)", `runtime/doc/scroll.txt`).
    ///
    /// Returns `None` when the cursor's own position already satisfies the
    /// invariant (the ordinary `FollowCursor` case — the viewport moved
    /// instead) or `Some(row)` — the row the CALLER must move the cursor
    /// to — when `mode` was `Independent` and the already-settled viewport
    /// left `cursor_row` outside the padded band.
    ///
    /// Converges in exactly one call with no intervening state change
    /// (`SYNC-IDEMPOTENT`): both branches leave `cursor_row` exactly on or
    /// inside `[new_top, new_bottom]`, so calling `reconcile` again with
    /// the same `cursor_row` (and the resulting `mode == FollowCursor`)
    /// is a no-op. See the effective_scrolloff doc for why the clamp is
    /// `(height - 1) / 2`, not `height / 2`.
    pub fn reconcile(&mut self, cursor_row: usize) -> Option<usize> {
        let height = self.height as usize;
        if height == 0 {
            self.mode = ScrollMode::FollowCursor;
            return None;
        }
        let off = self.effective_scrolloff();

        match self.mode {
            ScrollMode::FollowCursor => {
                let top = self.scroll_row + off;
                let bottom = self.scroll_row + height - 1 - off;
                if cursor_row < top {
                    self.scroll_row = cursor_row.saturating_sub(off);
                } else if cursor_row > bottom {
                    self.scroll_row = cursor_row + off + 1 - height;
                }
                None
            }
            ScrollMode::Independent => {
                self.mode = ScrollMode::FollowCursor;
                let top = self.scroll_row + off;
                let bottom = self.scroll_row + height - 1 - off;
                if cursor_row < top {
                    Some(top)
                } else if cursor_row > bottom {
                    Some(bottom)
                } else {
                    None
                }
            }
        }
    }
}

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
    /// "read-only" document — the exact Go bug `commands_clipboard.go:142-
    /// 152`'s comment describes, reintroduced by guarding the wrong layer).
    /// `commands::edit::undo`/`redo` deliberately do NOT check this field —
    /// Go's own `ApplyInverse`/`Reapply` (`edit_primitives.go:51,86`) bypass
    /// `m.readOnly` the same way, unlike `ReplaceRange`
    /// (`edit_primitives.go:25`) which checks it first.
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
    /// two trigger points — see `save::recompute_dirty`'s doc comment.
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
        }
    }

    /// Reads the render-only dirty cache — see `save::recompute_dirty`'s
    /// doc comment for the two points that keep it current.
    pub fn is_dirty(&self) -> bool {
        self.is_dirty_cached
    }

    /// Marks the freshly constructed buffer dirty relative to the file it
    /// was hydrated from — for `rune-cli::main`'s bootstrap ONLY, called (at
    /// most once, before the runtime loop and thus before `update` has ever
    /// run) when `rune-db`'s `Load` ack reports `recovered != disk_content`.
    pub fn mark_dirty_from_hydration(&mut self) {
        self.is_dirty_cached = true;
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
    /// switch over to its real name the moment it actually has one.
    pub fn bind_path(&mut self, path: PathBuf) {
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
        self.doc.sync_content(&self.buffer);
        self.doc.set_width(self.viewport.width);
        self.doc.sync_cursors(&self.buffer, &self.cursors);
        self.doc.snapshot(&self.buffer)
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
    pub fn scroll_to_cursor(&mut self, view: &ViewSnapshots) {
        let primary = self.cursors.primary();
        let buffer_point = self.buffer.offset_to_line_col(primary.position);
        let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
        if let Some(target_row) = self.viewport.reconcile(wrap_point.row) {
            self.snap_cursor_to_row(view, target_row);
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
        let syntax_point = view.wrap.wrap_to_syntax(WrapPoint { row, col });
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

    /// The fixed per-BATCH settle sequence: rebuild the view, then scroll to
    /// the (by now final) cursor exactly once. `App::sync_view` — called
    /// once per whole message batch by the runtime (`runtime::run`) and by
    /// tests that need the settled state — is the only caller; movement/
    /// editing commands call `view()` alone (see its docs).
    pub fn sync(&mut self) -> ViewSnapshots {
        let view = self.view();
        self.scroll_to_cursor(&view);
        view
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
        assert_eq!(first.display.total_rows, 3);
        let second = doc.sync();
        assert_eq!(second.display.total_rows, first.display.total_rows);
    }

    fn viewport(width: u16, height: u16) -> Viewport {
        Viewport {
            width,
            height,
            scroll_row: 0,
            scrolloff: 0,
            mode: ScrollMode::FollowCursor,
        }
    }

    #[test]
    fn reconcile_follow_cursor_keeps_row_in_view() {
        // scrolloff 0 reproduces the old `scroll_to_row` behaviour exactly.
        let mut vp = viewport(80, 5);
        assert_eq!(vp.reconcile(10), None);
        assert_eq!(vp.scroll_row, 6); // 10 + 1 - 5
        assert_eq!(vp.reconcile(2), None);
        assert_eq!(vp.scroll_row, 2); // scrolled back up to keep row 2 visible
    }

    #[test]
    fn reconcile_honours_scrolloff_margin() {
        let mut vp = viewport(20, 20);
        vp.scrolloff = 5;
        // Cursor at row 3 must be at least 5 rows from the top.
        assert_eq!(vp.reconcile(3), None);
        assert_eq!(vp.scroll_row, 0); // clamped: can't scroll above row 0
        assert_eq!(vp.reconcile(30), None);
        // top = scroll_row + 5, bottom = scroll_row + 20 - 1 - 5: row 30 must
        // land exactly on the bottom margin.
        assert_eq!(vp.scroll_row + 20 - 1 - 5, 30);
    }

    #[test]
    fn reconcile_converges_in_one_step() {
        // `SYNC-IDEMPOTENT` (rune-fuzz/src/invariant/render.rs): a second
        // `reconcile` call with the SAME cursor row must never move
        // `scroll_row` again.
        let mut vp = viewport(17, 23); // odd dimensions exercise the clamp
        vp.scrolloff = 5;
        for cursor_row in [0usize, 3, 11, 47, 199] {
            vp.reconcile(cursor_row);
            let scroll_before = vp.scroll_row;
            assert_eq!(
                vp.reconcile(cursor_row),
                None,
                "must not need a cursor snap"
            );
            assert_eq!(
                vp.scroll_row, scroll_before,
                "a second reconcile with the same cursor row moved scroll_row"
            );
        }
    }

    #[test]
    fn reconcile_independent_mode_snaps_the_cursor_never_the_viewport() {
        // A `commands::nav_scroll` command already moved `scroll_row` and
        // armed `Independent` mode; the viewport scrolled far enough away
        // that the (unmoved) cursor now sits outside the padded band —
        // `reconcile` must return the boundary row to snap the CURSOR to,
        // and must NOT move `scroll_row` itself (plan WP7.S1: "the cursor
        // is moved onto the window", not the other way around).
        let mut vp = viewport(10, 10);
        vp.scrolloff = 2;
        vp.scroll_row = 50;
        vp.mode = ScrollMode::Independent;
        let cursor_row = 0; // far above the new viewport
        let snapped = vp.reconcile(cursor_row);
        assert_eq!(
            vp.scroll_row, 50,
            "Independent mode must not move scroll_row"
        );
        assert_eq!(snapped, Some(52)); // top = scroll_row(50) + off(2)
        assert_eq!(vp.mode, ScrollMode::FollowCursor, "consumed exactly once");
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
