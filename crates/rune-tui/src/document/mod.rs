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

mod sync;
#[cfg(test)]
mod tests;

use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;

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
/// `pending_quit`/`messages`/`db_banner`/`should_quit` stay on `App`
/// (app-wide, not per-document); `pending_save_confirm` also stays on `App`
/// but is doc-tagged (`Option<(DocumentId, u32)>`) so a tab switch can't
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
    /// ⌘S still materializes bytes already typed, while ⌘Z (which would
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
    /// Never read directly by `is_dirty` — see `is_dirty_cached`.
    pub saved_version: u64,
    /// The bytes the last successful save/materialize ack actually
    /// persisted — ground truth dirtiness compares the LIVE
    /// buffer's content against THIS, not a version proxy: `Buffer::
    /// apply_edits` always returns `version + 1`, and undo/redo build a new
    /// buffer, so a version comparison alone leaves an edit-then-undo
    /// document dirty forever even though the bytes are back to identical.
    /// Advanced only by [`Document::finish_save_ok`]. `pub(crate)` (not
    /// `pub`, matching `is_dirty_cached`): only `finish_save_ok` may move the
    /// saved baseline, and a `pub` field left that invariant enforced by
    /// convention rather than the type system alone — an out-of-crate
    /// integration test that needs a dirty fixture goes through a real edit
    /// instead, see `dirty_common::force_dirty`.
    pub(crate) saved_content: Arc<str>,
    /// The save-lifecycle state a save-in-progress carries: the version/
    /// content it captured at `begin_save` time. Private — `begin_save`/
    /// `finish_save_ok`/`abandon_save` are the ONLY three places allowed to
    /// touch `save_in_flight`/`save_pending` together, so a save can never
    /// be in flight without the exact bytes it captured, and a stale
    /// capture can never survive to be promoted by a later unrelated ack.
    save_pending: Option<PendingSave>,
    pub save_in_flight: bool,
    /// The path an in-flight `bind_new` materialize is trying to CREATE
    /// (`save::bind_new_now`). Deliberately not `file_path`: a create that
    /// loses the no-clobber race must leave the draft untitled, or a later
    /// ⌘S would overwrite the winner. `handle_materialize_ack`
    /// moves it into `file_path` only once the write actually commits.
    pub pending_bind_path: Option<PathBuf>,
    /// The render-only dirty cache: `is_dirty` reads
    /// ONLY this field. Recomputed in `update`, and ONLY there, at exactly
    /// two trigger points — see `materialize_ack::recompute_dirty`'s doc comment.
    /// `pub(crate)` (not private) because the recompute chokepoint now lives
    /// in a different module (`save.rs`).
    pub(crate) is_dirty_cached: bool,
    /// The most recent display-pipeline snapshot, cached by `App::sync_view`
    /// for `render::draw` to blit. `None` only before this document's first
    /// sync.
    pub view: Option<ViewSnapshots>,
    /// This document's handle onto the app-wide recovery store — `AppDb`
    /// split into app-level `Db` and per-doc `DocDb`.
    /// `None` for a document with no recovery journal — an ephemeral/help
    /// document, or one opened before per-doc hydration exists (Assumption
    /// A1).
    pub db: Option<DocDb>,
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
    /// Which producer this document's content goes through —
    /// mirrored onto `doc` via `DocMachine::set_kind` every time it changes.
    /// Recomputed from `file_path` and the buffer's current content only
    /// inside `bind_path`, the single place a document acquires (or
    /// reacquires) a path; a pathless draft and the Help document therefore
    /// stay `DocumentKind::Markdown`, exactly as before this plan.
    pub kind: DocumentKind,
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
    /// This document's image state — `Some` only for a
    /// `DocumentKind::Image` document, populated by `workspace::open_bytes`
    /// at open time. `None` for every other document, including a document
    /// that used to be an image and no longer is (`kind` never changes back
    /// once bound — a document only ever acquires its `kind` once, at
    /// `bind_path`).
    pub image: Option<crate::graphics::ImageState>,
    /// This document's inline embed set — the several
    /// `![alt](x.png)`/`![[x.png]]` images a markdown document may hold at
    /// once, each independently spawned/decoded/transmitted/despawned.
    /// Distinct from `image` above (which describes a whole `DocumentKind::
    /// Image` document, exactly one image): the two are mutually exclusive
    /// in practice (an image document has no embeds; a markdown document is
    /// never `DocumentKind::Image`), but nothing enforces that at the type
    /// level — `embeds` just stays empty and `sync_embeds` a no-op for
    /// every document kind other than `Markdown`.
    pub embeds: crate::graphics::EmbedSet,
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
}

/// Why a document refuses mutation — not a plain bool, so a toggleable view
/// mode (`Reading`) can be told apart from a document with no editable form
/// at all (`Always`), and both from a transient, not-yet-committed one
/// (`Preview`): a toggle must not make the Help tab editable, and the
/// undo/redo guard and the `⌘S` footer hint each branch on the variants
/// differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOnly {
    /// Ordinary editable document.
    No,
    /// The user asked for reading view (⌃P) — the same chord returns it.
    /// The document keeps its journal, its `db` binding and any unsaved
    /// bytes.
    Reading,
    /// No editable form exists: the Help tab, the error banner, an image
    /// document. `commands::reading::toggle` refuses; only a mint site sets
    /// it.
    Always,
    /// A forthcoming Explorer feature previews the file under the cursor
    /// in the Editor without the user having committed to opening it —
    /// this document exists but has not been "opened" in the
    /// ordinary sense. Save, close, and rename all refuse it outright
    /// rather than acting on a document the user never asked to keep; a
    /// later work package flips it to `No` on promotion (the user actually
    /// editing it). Distinct from `Reading`: there is no chord that leaves
    /// `Preview` the way ⌃P leaves `Reading`, so undo/redo join `Reading`
    /// in refusing it rather than following `Always`'s bypass.
    Preview,
}

impl ReadOnly {
    /// The wording for why a read-only document refuses, or `None` for
    /// `No` — which refuses nothing, so it has no wording to give (carry
    /// that out of band instead of a sentinel string a missed check
    /// could pass off as real). `Reading` names the way out because the
    /// user reached it with a chord that also leaves it; `Always` has no
    /// way out to name. The one place both user-initiated refusal
    /// chokepoints (`App::refuse_if_read_only`) source their wording from.
    pub fn refusal_message(&self) -> Option<&'static str> {
        match self {
            ReadOnly::No => None,
            ReadOnly::Reading => Some("reading view — ⌃P to edit"),
            ReadOnly::Always => Some("this document is read-only"),
            ReadOnly::Preview => Some("preview — not yet open for editing"),
        }
    }
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
    /// which save must NOT (⌘S still materializes bytes already typed in
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
        self.image.is_some() || self.embeds.has_wedged()
    }
}

/// The save-in-progress capture: the exact version/bytes
/// [`Document::begin_save`] captured, held until the matching
/// [`Document::finish_save_ok`]/[`Document::abandon_save`] resolves it.
/// Private to this module — `Document`'s three chokepoint methods are the
/// only code that ever constructs, reads, or drops one.
struct PendingSave {
    version: u64,
    content: Arc<str>,
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
            read_only: ReadOnly::No,
            file_path: None,
            saved_version,
            saved_content,
            save_pending: None,
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
            image: None,
            embeds: crate::graphics::EmbedSet::new(),
            last_sync: None,
            pinned: false,
        }
    }

    /// Reads the render-only dirty cache — see `materialize_ack::recompute_dirty`'s
    /// doc comment for the two points that keep it current.
    pub fn is_dirty(&self) -> bool {
        self.is_dirty_cached
    }

    /// Arms a save in flight, capturing `version`/`content` TOGETHER at one
    /// chokepoint — the only way `save_in_flight` and `save_pending`
    /// are ever set, so a save can never be in flight without the exact
    /// bytes it captured. Called by every save-start site: `save::
    /// materialize_now`, the no-store fallback in `save::trigger_save`, and
    /// `save::bind_new_now`.
    pub fn begin_save(&mut self, version: u64, content: Arc<str>) {
        self.save_in_flight = true;
        self.save_pending = Some(PendingSave { version, content });
    }

    /// Resolves a successful save ack: clears in-flight, and promotes the
    /// captured `save_pending` content into `saved_content` iff its version
    /// matches `version` (the ack's own correlated fact) AND exceeds
    /// `saved_version` — so an ack for a capture this document no longer
    /// recognizes (or one that would move the baseline BACKWARD) promotes
    /// nothing. Returns whether the promotion happened. The only writer of
    /// `saved_content`/`saved_version`.
    pub fn finish_save_ok(&mut self, version: u64) -> bool {
        self.save_in_flight = false;
        let Some(pending) = self.save_pending.take() else {
            return false;
        };
        if pending.version == version && pending.version > self.saved_version {
            self.saved_version = pending.version;
            self.saved_content = pending.content;
            true
        } else {
            false
        }
    }

    /// Resolves a failed/abandoned save: clears in-flight and drops the
    /// captured bytes without promoting anything — the exact opposite of
    /// `finish_save_ok`. Every clear site that isn't a genuine success
    /// (`fail_materialize_locally`, `on_store_failure`'s sweep,
    /// `bind_new_now`'s error arm, `handle_save_done`'s `Err` arm) goes
    /// through here, never through a direct field write.
    pub fn abandon_save(&mut self) {
        self.save_in_flight = false;
        self.save_pending = None;
    }

    /// The version an in-flight save captured, if one is running — a
    /// read-only peek (never mutates) for `materialize_ack::
    /// handle_materialize_ack`'s ack-side chokepoint, which needs it to
    /// correlate `MatResult` (which carries no version of its own) against
    /// the SAME bytes `begin_save` captured before deciding whether to call
    /// `finish_save_ok` or `abandon_save`.
    pub fn pending_save_version(&self) -> Option<u64> {
        self.save_pending.as_ref().map(|p| p.version)
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
    /// unannotated no-op. Dirtiness is no longer marked here: an
    /// adopting hydration leaves the buffer genuinely different from
    /// `saved_content`, so the ordinary content comparison already reports
    /// it dirty once the caller recomputes — see `materialize_ack::
    /// recompute_dirty`, called after every hydration site.
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
        let kind = kind_for(Some(&path), self.buffer.content());
        self.kind = kind;
        self.doc.set_kind(self.kind);
        self.file_path = Some(path);
        self.display_name = None;
    }
}
