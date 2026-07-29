//! `App`: the Elm-style model. `update` is the ONLY writer of synchronous
//! state (CONSTITUTION §5.4: "mutate synchronous state directly in
//! `update`; a Cmd is exclusively for I/O that leaves the thread").
//!
//! Plan WP1 reshaped `App` around a `DocumentId`-keyed map of `Document`s
//! (decision 1/2): everything that used to live directly on `App` but is
//! really per-editing-pane state (file identity, save/dirty bookkeeping,
//! the display-pipeline cache, the per-doc recovery-store handle) moved
//! onto `Document` (`document.rs`). What's left here is genuinely app-wide:
//! the document map itself, the shared `Vfs`, the shared recovery `Store`
//! handle, and UI chrome state that spans every document (status message,
//! quit-confirm arming, the degraded-store banner).

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Vfs;

use crate::banner::Modal;
use crate::db::Db;
use crate::dispatch;
use crate::document::{Document, DocumentId, DocumentMap};
use crate::explorer::Explorer;
use crate::keymap::QuitKey;
use crate::opentabs::OpenTabs;
use crate::pane::Pane;
use crate::runtime::{Effects, Msg};
use crate::save;

/// Which subsystem last wrote `App::status_message` — the provenance tag
/// `Msg::SaveDone`'s success arm needs so it clears ONLY a message its own
/// save path set, never an unrelated one (review finding F2: an earlier
/// version cleared `status_message` unconditionally on every successful
/// save, stomping e.g. an unresolved "pbpaste failed" error the user hadn't
/// dismissed yet). The ORIGINAL status-message ownership rule (F2 in
/// `commands::edit`: a successful edit/undo/redo must never clear an
/// unrelated message) still holds unchanged — those call sites only ever
/// WRITE `status_message`, they never clear it, so they need no provenance
/// tag for that rule; they still tag their writes below so a stale write
/// from one of them can never be mistaken for `SaveError` and get swept up
/// by a later successful save.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusSource {
    /// A failed (or un-attempted, e.g. "no file to save") save attempt —
    /// the ONLY source a successful `Msg::SaveDone` is allowed to clear.
    SaveError,
    /// Everything else: edit/undo/redo failures, the degraded-save confirm
    /// hint, ... `Msg::Error` no longer writes `status_message` at all (plan
    /// WP3.S4: routed through `banner::report_error`, a modal, instead).
    #[default]
    Other,
}

/// The whole editor model: a `DocumentId`-keyed map of every open document,
/// the injected `Vfs` save target shared by all of them, this session's
/// recovery store (app-level half), and app-wide UI state (status message,
/// quit-confirm arming) that doesn't belong to any one document.
pub struct App {
    pub documents: DocumentMap,
    pub active: DocumentId,
    next_doc_id: NonZeroU64,
    pub vfs: Arc<dyn Vfs + Send + Sync>,
    /// Which chrome region owns the next keystroke once `GLOBAL_BINDINGS`
    /// doesn't claim it (decision 7/8, `pane.rs`) — defaults to `Editor`.
    pub focus: Pane,
    /// Whether the Explorer/Open-Tabs left column is showing (decision 7);
    /// `false` by default (pre-WP2 geometry) until `^x` shows it.
    pub left_visible: bool,
    /// The terminal's last-known RAW row count, as reported by the most
    /// recent `Msg::Resize` — unlike the active document's own `viewport.
    /// height` (which `Msg::Resize` sets to `height - 1`, reserving the
    /// footer row), this is the exact `frame.area().height` `render::draw`
    /// itself sizes the banner against. `App::sync_view` threads this into
    /// `banner::sync_modal` (mirroring how it already threads the active
    /// document's `viewport.width` through) so `banner::banner_height` — the
    /// one function both `render::draw` and `sync_modal` call — computes the
    /// identical figure in both places. `0` before the first `Msg::Resize`
    /// (never observed in practice: `runtime::run` seeds one before the
    /// first `sync_view`/draw).
    pub frame_height: u16,
    /// The terminal's last-known RAW column count, alongside `frame_height`
    /// above — same provenance (the most recent `Msg::Resize`), same reason
    /// it exists separately from any document's own `viewport.width`: once
    /// `relayout` sizes the viewport from the EDITOR rect (which may be
    /// narrower than the frame — the left pane, a border), `viewport.width`
    /// is no longer the full frame width, so anything that needs the frame's
    /// own width (`banner::sync_modal`, `layout::geometry` itself) reads
    /// this instead. `0` before the first `Msg::Resize`, exactly like
    /// `frame_height`. `App::relayout` is a no-op while this is `0`.
    pub frame_width: u16,
    /// The Explorer pane's own state (plan WP4.S3): root, listing, cursor.
    /// Starts unloaded; `pane::handle_global_command` loads it on `^x`.
    pub explorer: Explorer,
    /// The Open Tabs pane's own state (plan WP5.S1): tab display order and
    /// its cursor/scroll position — kept in sync with `documents` at its own
    /// chokepoints (`App::open_document`/`workspace::close_now`).
    pub tabs: OpenTabs,
    /// The Help virtual document's id, once minted (plan WP7.S2) — `None`
    /// until the first `F1`. Makes `workspace::toggle_help` idempotent (a
    /// second press never mints a duplicate).
    pub help_doc: Option<DocumentId>,
    /// The document active right before `F1` last activated Help —
    /// `workspace::toggle_help`'s target when toggling back off.
    pub help_return_to: Option<DocumentId>,
    /// The editable title field (`title.rs`) — the file name a rename types
    /// into. Reseeded at every document switch (`workspace::switch_to`) and
    /// at every focus gain (`pane::focus_title`), so it always describes
    /// the document actually showing. Unjournaled (§12).
    pub title: crate::title::TitleField,
    /// The rename workflow's state (`rename.rs`) — a typed machine rather
    /// than another ad hoc pending-boolean, because `[R]eplace` has a
    /// mid-sequence point past which it is not cancellable (§1.4.10).
    pub rename: crate::rename::RenameState,
    /// `pub(crate)`: `rename.rs` is the sole minter of new generations for
    /// its own `Cmd` route (mirrors `next_quit_gen`/`next_save_confirm_gen`).
    pub(crate) next_rename_gen: u32,
    pub status_message: Option<String>,
    /// Provenance of `status_message` — see `StatusSource`'s docs. Only
    /// meaningful while `status_message.is_some()`; a later `set_status`
    /// call always updates both fields together, so a stale value here
    /// after the message is cleared can never be observed.
    pub status_source: StatusSource,
    /// This session's recovery store (plan WP1 decision 5: split out of the
    /// pre-WP1 `AppDb` — this is the app-wide half, shared by every open
    /// document; each document's own binding lives in its `Document::db:
    /// Option<DocDb>`). `None` only when no store could be constructed at
    /// all (an extreme fallback distinct from `Db::degraded`, which still
    /// has a live, if untrusted, store).
    pub db: Option<Db>,
    /// Correlates an in-flight `rune-db` op id to the `DocumentId` that
    /// enqueued it (plan WP1 decision 6) — inserted at every successful
    /// `Store` enqueue (`db::append_edit`/`move_undo_pos`/`db::
    /// load_document`/`save::materialize_now`/`save::handle_snapshot_due`),
    /// removed by `handle_db_event` once its ack lands. Needed because the
    /// writer thread's single FIFO ack stream has no per-document identity
    /// of its own once more than one document can enqueue.
    pub db_ops: HashMap<u64, DocumentId>,
    /// The issuing document's `buffer.version()` at the moment a `Load` op
    /// was enqueued (`db::load_document`), keyed by that op's id — popped
    /// alongside `db_ops` by `handle_db_event`'s `Load` arm. `Load` is
    /// asynchronous, so the user may type into the buffer during the round
    /// trip; comparing this recorded version against the buffer's version
    /// AT ACK TIME is how `db::handle_load_ack` decides whether adopting
    /// the ack's recovered content would silently clobber those keystrokes
    /// (never clobber keystrokes to complete a recovery binding).
    pub db_load_versions: HashMap<u64, u64>,
    /// WP7: correlates an in-flight `MaterializeRecord` op id to the
    /// document whose disk write ALREADY physically completed before this
    /// op was even enqueued — the caller-side vfs work runs first, this
    /// bookkeeping op runs after. A dead writer failing precisely THIS op
    /// (`DbEvent::Err`/`Fatal`) must never be reported as a failed save
    /// ([rune-db 1]): `handle_db_event` consults this map to react with a
    /// synthetic committed ack instead of the ordinary failure path.
    /// Cleared on both success and failure — never left stale.
    pub published_ops: HashMap<u64, DocumentId>,
    /// WP7: the content/path/CAS facts a `materialize` attempt captured at
    /// trigger time, held here between `MaterializePrepare`'s ack (which
    /// carries no disk-sourced data at all) and the caller-side `vfs` `Cmd`
    /// it spawns — `save::PendingMaterialize`'s doc comment explains why
    /// each field is captured once and never re-derived (§1.4.2/§1.4.8).
    pub(crate) pending_materialize: HashMap<DocumentId, crate::save::PendingMaterialize>,
    /// A persistent status banner independent of `status_message`'s
    /// provenance-cleared slot (plan WP5.S2/S3: "persistent status banner")
    /// — set once the store degrades (at open, or from a later
    /// `on_store_failure`) and never cleared automatically.
    pub db_banner: Option<String>,
    /// The armed degraded-save confirm chord's target document and timer
    /// generation — `None` when no confirm is pending (plan WP1 decision 3:
    /// doc-tagged so a tab switch can't misapply an armed confirm gate;
    /// mirrors `pending_quit` below). Stale `SaveConfirmTimeout` generations
    /// are ignored.
    pub pending_save_confirm: Option<(DocumentId, u32)>,
    /// `pub(crate)`, not private: `save::trigger_save` — a different module
    /// since the WP1.S5 extraction — is the sole minter of new generations.
    pub(crate) next_save_confirm_gen: u32,
    /// The document a Guard modal's `[S]ave` armed a save-then-close for
    /// (plan WP5.S3) — `None` when no close is waiting on a save ack.
    /// `banner::handle_key`'s Guard arm sets this immediately before
    /// calling `save::trigger_save`; `save::handle_materialize_ack`/
    /// `handle_save_done`'s success paths are the only readers, closing
    /// the document (`workspace::close_now`) only when the id still
    /// matches AND the save actually committed — a failed save leaves the
    /// document open with its usual error surfaced instead.
    pub pending_close_on_save: Option<DocumentId>,
    /// The armed quit chord and its timer generation — `None` when no quit
    /// is pending. Stale `ConfirmTimeout` generations are ignored (plan
    /// Context, "Quit-confirm"). App-wide, not doc-tagged: quitting closes
    /// the whole session, not one document.
    pub pending_quit: Option<(QuitKey, u32)>,
    /// `pub(crate)`: `pane::handle_quit_key` (moved out of this module in
    /// WP2) is the sole minter of new generations.
    pub(crate) next_quit_gen: u32,
    /// Answers "is the spacebar physically down right now?" for the held-
    /// space leader (plan WP5.S3/decision 3). Defaults to `NullProbe`
    /// (always `false`) so the leader is inert unless something opts in:
    /// `rune-cli::main` installs the real `HidSpaceProbe`, tests install
    /// `FixedSpaceProbe`, and the fuzzer keeps the default and so stays
    /// deterministic.
    pub space_probe: Box<dyn crate::keystate::SpaceProbe>,
    /// The click-aggregation + drag-selection state a mouse gesture needs
    /// across messages (plan WP7.S5) — `commands::mouse`'s sole owner.
    pub pointer: crate::pointer::PointerState,
    /// Answers "what time is it right now?" for `pointer`'s multi-click
    /// window (plan WP7.S5: "inject the clock as a field so the fuzzer can
    /// reproduce a gesture"), mirroring `space_probe` above: production
    /// installs the real wall clock, tests install a `ManualClock`.
    pub pointer_clock: Box<dyn crate::pointer::Clock>,
    /// Which binding set governs the editor pane (plan WP6.S8) — defaults to
    /// `BindingSet::Default` (the VS Code-style set this crate has had since
    /// WP2). `app::handle_editor_key` does not consult this yet: full vim
    /// modal editing is out of scope for this plan (see `keymap::vim`'s doc
    /// comment); this field exists so a future dispatch switch has
    /// somewhere to read from.
    pub binding_set: crate::keymap::BindingSet,
    /// The document a literal space was just typed into, armed by the
    /// printable-insert path (`handle_editor_key`) and cleared at the top
    /// of the NEXT `handle_key` (plan WP5.S4/S5). `Some` for exactly one
    /// keystroke, so a stale arming is unrepresentable rather than guarded
    /// against.
    pub speculative_space: Option<DocumentId>,
    /// The single modal slot (plan WP3, decision 13): `Some` while an error
    /// banner (WP5: or a close-guard prompt) is up. `banner::set_modal` is
    /// the one chokepoint that writes a NEW modal here (plan Risks, "Banner
    /// reentrancy"); stage 1's `Esc`/`c` handling is the only other writer.
    pub modal: Option<Modal>,
    pub should_quit: bool,
    /// The rendered theme (plan WP4 Half 2) — the one `Theme` every chrome
    /// style and every markdown/code `ScopeId` in this app resolves
    /// through; nothing in `render.rs`/`explorer.rs`/`footer.rs`/etc. reads
    /// a raw indexed- or truecolor literal directly (those live only under
    /// `theme/`). `App::new` defaults to truecolor (Catppuccin Mocha,
    /// unquantized) — production
    /// startup (`rune-cli`) overwrites it once `term::Guard` exists and
    /// `theme::probe::supports_truecolor` can actually query the real
    /// terminal; every test and the fuzzer keep this default, exactly like
    /// `space_probe`'s `NullProbe` default above.
    pub theme: crate::theme::Theme,
    /// The workspace root discovered by `workspaceroot::resolve` (plan
    /// WP4.S4) — the nearest ancestor of the launch directory carrying a
    /// `.git`/`.obsidian` marker, or `cwd` itself when none is found.
    /// `PathBuf::new()` (empty) until `set_root` runs; an empty root is a
    /// legal "not yet resolved" state, and every consumer (the Explorer's
    /// initial-root fallback, the breadcrumb's relativization) skips it
    /// rather than treating it as a real path.
    pub root: PathBuf,
    /// The snapshot-autosave debounce's one rearmable timer (plan WP16.S5)
    /// — `pub(crate)`, not private: `save::schedule_snapshot_debounce` (a
    /// different module) is the sole caller of `arm`. No background thread
    /// exists until `runtime::run` calls `attach` on it, so a test/fuzz
    /// `App` that never reaches that loop never spawns one (mirrors
    /// `db.bridge`'s own bootstrap/live split).
    pub(crate) snapshot_timer: Arc<crate::runtime::SnapshotTimer>,
}

impl App {
    pub fn new(
        buffer: Buffer,
        file_path: Option<PathBuf>,
        vfs: Arc<dyn Vfs + Send + Sync>,
        db: Option<Db>,
    ) -> App {
        let mut document = Document::new(buffer);
        if let Some(path) = file_path {
            document.bind_path(path);
        }

        let id = DocumentId(NonZeroU64::MIN);
        let documents = DocumentMap::new(id, document);

        App {
            documents,
            active: id,
            next_doc_id: NonZeroU64::MIN.saturating_add(1),
            vfs,
            focus: Pane::Editor,
            left_visible: false,
            frame_height: 0,
            frame_width: 0,
            explorer: Explorer::default(),
            tabs: OpenTabs::new(id),
            help_doc: None,
            help_return_to: None,
            title: crate::title::TitleField::default(),
            rename: crate::rename::RenameState::default(),
            next_rename_gen: 0,
            status_message: None,
            status_source: StatusSource::Other,
            db,
            db_ops: HashMap::new(),
            db_load_versions: HashMap::new(),
            published_ops: HashMap::new(),
            pending_materialize: HashMap::new(),
            db_banner: None,
            pending_save_confirm: None,
            next_save_confirm_gen: 0,
            pending_close_on_save: None,
            pending_quit: None,
            next_quit_gen: 0,
            space_probe: Box::new(crate::keystate::NullProbe),
            pointer: crate::pointer::PointerState::default(),
            pointer_clock: Box::new(crate::pointer::SystemClock),
            binding_set: crate::keymap::BindingSet::default(),
            speculative_space: None,
            modal: None,
            should_quit: false,
            theme: crate::theme::Theme::catppuccin_mocha(false),
            root: PathBuf::new(),
            snapshot_timer: crate::runtime::SnapshotTimer::new(),
        }
    }

    /// The default entry point for a `rune` launch with no file argument
    /// (`rune-cli::main`): an empty, pathless draft whose `display_name` is
    /// overridden to "Untitled 1" so the title row reads that instead of
    /// the generic `"[No Name]"` placeholder every OTHER pathless document
    /// falls back to (`Document::file_name`). Kept as its own constructor,
    /// rather than inline in the binary crate, so this bootstrap shape is
    /// exercisable from `rune-tui`'s own test harnesses — `rune-cli` has no
    /// `tests/` directory of its own.
    ///
    /// Always opens with `db: None` — a brand-new document has no
    /// `documents` row for `rune-db` to hydrate yet (see `crates/rune-tui/
    /// TODO.md`, "no recovery journal for the default untitled document").
    pub fn new_untitled(vfs: Arc<dyn Vfs + Send + Sync>) -> App {
        let mut app = App::new(Buffer::new(""), None, vfs, None);
        app.active_doc_mut().display_name = Some("Untitled 1".to_string());
        app
    }

    /// Mints the next `DocumentId`, monotonically — never reused, even
    /// across a close (out of scope for WP1; the counter design already
    /// supports it). `saturating_add` rather than `wrapping_add`: wrapping
    /// back to a low id could collide with one still live in `documents`,
    /// silently aliasing two documents — saturating at `u64::MAX` instead
    /// (a session would need to open ~2^64 documents to ever reach it) just
    /// stops minting new ones, never hands out a reused id.
    fn mint_doc_id(&mut self) -> DocumentId {
        let id = DocumentId(self.next_doc_id);
        self.next_doc_id = self.next_doc_id.saturating_add(1);
        id
    }

    /// Inserts a new, not-yet-active `Document` into the map and returns its
    /// id. The minimal multi-document seam WP1 needs (its own unit tests in
    /// `db.rs`/`save.rs`, "two docs enqueue ops") — full open/close/switch
    /// UX (`workspace::open_path`) is WP4/WP5 scope.
    pub fn open_document(&mut self, buffer: Buffer) -> DocumentId {
        let id = self.mint_doc_id();
        self.documents.insert(id, Document::new(buffer));
        // The Open Tabs chokepoint (plan WP5.S1): every document, however
        // it was opened, gets a tab the moment it exists.
        self.tabs.order.push(id);
        id
    }

    /// Looks up `id` — `None` if it doesn't reference a live document.
    /// `workspace::close_now` removes entries, so this is exactly the shape
    /// a stale id from `App::db_ops` racing a close must produce: a plain,
    /// honest "not found" a caller can drop, never a silent write to some
    /// OTHER document. Callers that specifically want the active document
    /// use `active_doc`/`active_doc_mut` instead, which are infallible.
    pub fn doc(&self, id: DocumentId) -> Option<&Document> {
        self.documents.get(&id)
    }

    pub fn doc_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.documents.get_mut(&id)
    }

    /// Infallible by construction, not by convention: `DocumentMap` (plan
    /// WP5.S5) guarantees at least one entry always exists, so "`active`
    /// names a removed document" falls back to that guaranteed entry
    /// instead of needing a `#[allow(clippy::unwrap_used)]` escape hatch —
    /// `workspace::close_now` still reassigns `active` to a real neighbor
    /// before removing, so this fallback is never actually exercised in
    /// practice, but it is real code with a real answer, not a masked
    /// panic.
    pub fn active_doc(&self) -> &Document {
        self.documents.get_or_anchor(&self.active)
    }

    pub fn active_doc_mut(&mut self) -> &mut Document {
        self.documents.get_or_anchor_mut(&self.active)
    }

    /// Convenience delegate to the active document's dirty cache
    /// (CONSTITUTION §1.4.8) — kept on `App` so render/status call sites
    /// don't need to spell out `app.active_doc().is_dirty()` everywhere.
    pub fn is_dirty(&self) -> bool {
        self.active_doc().is_dirty()
    }

    pub fn file_name(&self) -> &str {
        self.active_doc().file_name()
    }

    /// See `Document::mark_dirty_from_hydration`'s docs — delegates to the
    /// (sole, at bootstrap time) active document.
    pub fn mark_dirty_from_hydration(&mut self) {
        self.active_doc_mut().mark_dirty_from_hydration();
    }

    /// The ONE geometry chokepoint's writer (plan WP3 decision 1/2): derives
    /// every frame rect from `layout::geometry` and sizes the ACTIVE
    /// document's viewport from its `editor` rect. A no-op while either
    /// frame dimension is still `0` (before the first `Msg::Resize` —
    /// `layout::geometry` would otherwise be asked to lay out a
    /// zero-by-something frame it never actually has to render).
    ///
    /// Called as the first statement of `sync_view` below (the runtime
    /// calls `sync_view` immediately before every `render::draw`, so that's
    /// the one chokepoint no call site can forget) AND
    /// again from `Msg::Resize` itself, so tests that call `update` without
    /// a following `sync_view` still see a correctly-sized viewport;
    /// calling it twice in the same message batch is harmless (it's a pure
    /// function of `frame_width`/`frame_height`, idempotent either way).
    ///
    /// `.max(1)` on both dimensions (plan gotcha 13): the fuzzer drives
    /// `Resize` down to a 1x2 frame, and a 0-width/0-height viewport would
    /// reach `Document::set_width`'s wrap engine with a wrap column of `0`.
    pub fn relayout(&mut self) {
        if self.frame_width == 0 || self.frame_height == 0 {
            return;
        }
        let area = ratatui::layout::Rect::new(0, 0, self.frame_width, self.frame_height);
        let geo = crate::layout::geometry(area, self);
        let (w, h) = (geo.editor.width.max(1), geo.editor.height.max(1));
        self.active_doc_mut().viewport.set_size(w, h);
    }

    /// Re-runs the display pipeline for the ACTIVE document and caches the
    /// result on it for `render::draw` to blit. Safe to call more than once
    /// per message batch — see `Document::sync`'s docs. Only the active
    /// document is synced (Phase 1/WP1: exactly one document is ever
    /// visible) — a later multi-pane WP re-evaluates this against whichever
    /// documents are actually on screen.
    ///
    /// Derives the active document's `focused` flag from `App::focus` FIRST,
    /// every call (plan Gotchas: `&& app.modal.is_none()` — a modal up means
    /// the editor is never really focused). Also re-syncs the modal
    /// document, if one is up (WP3.S3), at the terminal's own width — kept
    /// in this settle step, never inside `render::draw` itself (§5.4).
    pub fn sync_view(&mut self) {
        self.relayout();
        let focused = self.focus == Pane::Editor && self.modal.is_none();
        self.active_doc_mut().focused = focused;
        let view = self.active_doc_mut().sync();
        self.active_doc_mut().view = Some(view);
        if self.modal.is_some() {
            let width = self.frame_width;
            let frame_height = self.frame_height;
            crate::banner::sync_modal(self, width, frame_height);
        }
    }

    /// The single writer of a NEW `status_message`: every call site that
    /// wants to set one goes through here instead of writing
    /// `status_message`/`status_source` separately, so the text and its
    /// provenance tag (`StatusSource`) can never drift apart.
    pub fn set_status(&mut self, message: impl Into<String>, source: StatusSource) {
        self.status_message = Some(message.into());
        self.status_source = source;
    }

    /// Records the workspace root discovered by `workspaceroot::resolve`
    /// (plan WP4.S4) — the one writer of `root`, called once at startup once
    /// the launch-time `cwd`/`home`/file-argument inputs the resolver needs
    /// are available.
    pub fn set_root(&mut self, root: PathBuf) {
        self.root = root;
    }
}

/// The ONLY writer of `App` state (§5.4). `effects` accumulates I/O for the
/// runtime loop to perform after the whole message batch is applied:
/// `effects.raw` for OSC 52 (drained by the main loop, never a `Cmd` — plan
/// Gotchas, "Cmds must never touch the terminal"), `effects.cmds` for
/// off-thread work (save, pbpaste, the quit-confirm/save-confirm/snapshot
/// timers).
///
/// Wraps `update_inner` with the ONE chokepoint for the snapshot-autosave
/// debounce (plan WP5.S6): every message that mutates the ACTIVE document's
/// `journal` — typing, undo/redo, cut, paste, ... — funnels through
/// `commands::edit::commit_edit_batch`/`undo`/`redo`, so comparing the
/// active document's journal position before and after `update_inner`
/// catches all of them uniformly, without threading a debounce call through
/// every editing command's call site individually.
pub fn update(app: &mut App, msg: Msg, effects: &mut Effects) {
    let journal_pos_before = app.active_doc().journal.pos();
    let active_before = app.active;
    let buffer_version_before = app.active_doc().buffer.version();
    dispatch::update_inner(app, msg, effects);
    if app.active_doc().journal.pos() != journal_pos_before {
        let id = app.active;
        save::schedule_snapshot_debounce(app, id);
    }
    // Plan WP5.S3: the active buffer changed shape, or the active document
    // itself did (a tab switch) — either way it may still need a highlight.
    if app.active != active_before || app.active_doc().buffer.version() != buffer_version_before {
        let id = app.active;
        crate::highlight::schedule_highlight(app, id, effects);
    }
}

// `schedule_highlight` moved to `highlight.rs` (§1.6 budget) — `update`
// above calls it through `highlight::`.

// `update_inner` (the top-level `Msg` dispatch), `handle_key`,
// `handle_editor_key` and `handle_db_event` moved to `dispatch.rs` (§1.6
// budget) — `update` above calls the first through `dispatch::`.

// `handle_quit_key` and its 2s timer `Cmd` moved to `pane.rs` in WP2 (§1.6
// budget) — `GlobalCommand::QuitChord` is its only remaining caller. Their
// unit tests moved to `tests/app_quit_and_dispatch.rs` earlier (WP1.S5).
