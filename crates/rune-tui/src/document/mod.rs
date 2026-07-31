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
//!
//! Split for the §1.6 budget: this file holds the type itself plus
//! construction/identity/hydration/save-state bookkeeping; [`sync`] holds
//! the view/scroll/settle sequence (`view`, `sync_catalogue`,
//! `scroll_to_cursor`, `sync`) as a second `impl Document` block, since none
//! of that sequence depends on anything declared only in this file.

mod sync;
#[cfg(test)]
mod tests;

use std::num::NonZeroU64;
use std::path::PathBuf;

use rune_core::buffer::{Buffer, Edit};
use rune_core::cursor::CursorSet;
use rune_core::undo::{Journal, Step};
use rune_md::element::doc::{DocMachine, ViewSnapshots};
use rune_md::icons::IconSet;
use rune_syntax::DocumentKind;

use crate::db::DocDb;
pub use crate::document_support::Hydration;
use crate::document_support::{is_suspicious_shrink, kind_for};
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
    /// The icon tier line decorations render with (plan WP5) — mirrored
    /// onto `doc` via `DocMachine::set_icons` on every `view()` call, same
    /// pattern as `kind`/`set_kind`. `Document` holds no `App` reference,
    /// so this is a plain field an outside writer (`App::sync_view`, the
    /// same chokepoint that pushes `focused` down before every sync) sets
    /// from the one `App`-held decision (`App::icons`) rather than a value
    /// this type could ever derive on its own.
    pub icons: IconSet,
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
            icons: IconSet::unicode(),
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
}
