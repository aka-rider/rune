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
use std::path::{Path, PathBuf};

use rune_core::buffer::Buffer;
use rune_core::coords::WrapPoint;
use rune_core::cursor::{Cursor, CursorSet};
use rune_core::undo::Journal;
use rune_md::element::doc::{DocMachine, ViewSnapshots};
use rune_syntax::DocumentKind;

use crate::db::DocDb;

mod viewport;
pub use viewport::{ScrollMode, Viewport};

/// Derives the producer a path should use (plan WP4.S4): no path at all
/// (an untitled draft) or a `.md` extension stays `Markdown`; an extension
/// `rune_ts::lang::resolve` recognises becomes `Code`; anything else is
/// `Plain`. Deliberately calls `lang::resolve`, never `registry()` — the
/// former is a pure `&'static` table lookup with no tree-sitter call at
/// all, so no query compilation happens on this (the UI) thread.
fn kind_for(path: Option<&Path>) -> DocumentKind {
    let Some(path) = path else {
        return DocumentKind::Markdown;
    };
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("md") => DocumentKind::Markdown,
        Some(ext) => match rune_ts::lang::resolve(ext) {
            Some(name) => DocumentKind::Code(name),
            None => DocumentKind::Plain,
        },
        None => DocumentKind::Plain,
    }
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
    /// Which producer this document's content goes through (plan WP4) —
    /// mirrored onto `doc` via `DocMachine::set_kind` every time it changes.
    /// Recomputed from `file_path` only inside `bind_path`, the single place
    /// a document acquires (or reacquires) a path; a pathless draft and the
    /// Help document therefore stay `DocumentKind::Markdown`, exactly as
    /// before this plan.
    pub kind: DocumentKind,
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
            kind: DocumentKind::Markdown,
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
        if let Some(target_row) = self.viewport.reconcile(display_row) {
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
        assert_eq!(first.display.total_rows(), 3);
        let second = doc.sync();
        assert_eq!(second.display.total_rows(), first.display.total_rows());
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
