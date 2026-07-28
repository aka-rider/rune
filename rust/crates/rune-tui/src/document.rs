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
use rune_core::cursor::CursorSet;
use rune_core::undo::Journal;
use rune_md::element::doc::{DocMachine, ViewSnapshots};

use crate::db::DocDb;

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
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            width: 80,
            height: 24,
            scroll_row: 0,
        }
    }
}

impl Viewport {
    pub fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    /// Clamp `scroll_row` so wrap row `row` is visible — the scroll-to-
    /// cursor step of the per-message sync sequence.
    pub fn scroll_to_row(&mut self, row: usize) {
        let height = self.height as usize;
        if height == 0 {
            return;
        }
        if row < self.scroll_row {
            self.scroll_row = row;
        } else if row >= self.scroll_row + height {
            self.scroll_row = row + 1 - height;
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
        self.viewport.scroll_to_row(wrap_point.row);
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

    #[test]
    fn scroll_to_row_keeps_row_in_view() {
        let mut vp = Viewport {
            width: 80,
            height: 5,
            scroll_row: 0,
        };
        vp.scroll_to_row(10);
        assert_eq!(vp.scroll_row, 6); // 10 + 1 - 5
        vp.scroll_to_row(2);
        assert_eq!(vp.scroll_row, 2); // scrolled back up to keep row 2 visible
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
