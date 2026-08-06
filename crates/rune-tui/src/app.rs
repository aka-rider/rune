//! `App`: the Elm-style model. `update` is the ONLY writer of synchronous
//! state — mutate synchronous state directly in `update`; a Cmd is
//! exclusively for I/O that leaves the thread.
//!
//! `App` is shaped around a `DocumentId`-keyed map of `Document`s:
//! everything that used to live directly on `App` but is
//! really per-editing-pane state (file identity, save/dirty bookkeeping,
//! the display-pipeline cache, the per-doc recovery-store handle) moved
//! onto `Document` (`document.rs`). What's left here is genuinely app-wide:
//! the document map itself, the shared `Vfs`, the shared recovery `Store`
//! handle, and UI chrome state that spans every document (status message,
//! quit-confirm arming, the degraded-store banner).

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Vfs;

use crate::db::Db;
use crate::dispatch;
use crate::document::{Document, DocumentId};
use crate::document_map::DocumentMap;
use crate::explorer::Explorer;
use crate::guard::GuardPrompt;
use crate::keymap::QuitKey;
use crate::messages::MessageLog;
use crate::opentabs::OpenTabs;
use crate::pane::Pane;
use crate::runtime::{Effects, Msg};
use crate::save;

/// The quit-save fan-out `GuardKind::DirtyQuit`'s `[S]ave` answer arms:
/// every document a save was actually started for, each keyed to the
/// buffer version THAT save captured (`Document::pending_save_version`'s own
/// value at the moment `trigger_save` returned `SaveStart::InFlight`).
/// Emptying the map — every entry retired by a matching successful ack, or
/// by the document closing out from under the wait — is what flips
/// `should_quit`; any save failing outright aborts the whole intent instead
/// (never exit over a save the user believes succeeded).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuitIntent {
    pub pending: std::collections::BTreeMap<DocumentId, u64>,
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
    /// doesn't claim it (`pane.rs`) — defaults to `Editor`. `pub(crate)`,
    /// not private: the writers (`focus_title`/`refocus_title`/`set_focus`,
    /// `focus.rs`'s own `impl App` block — moved there to keep this file
    /// under the 500-line budget) live in a different module now, but they
    /// remain the ONLY code in this crate that assigns it directly; every
    /// other call site goes through `focus()`/`set_focus_pane`. Leaving the
    /// title always runs through the one commit chokepoint
    /// (`title::on_blur`).
    pub(crate) focus: Pane,
    /// The draggable splitter positions sizing the left column and its
    /// Explorer/Tabs division; starts hidden.
    pub splits: crate::layout::Splits,
    /// The terminal's last-known RAW row count, as reported by the most
    /// recent `Msg::Resize` — unlike the active document's own `viewport.
    /// height` (which `Msg::Resize` sets to `height - 1`, reserving the
    /// footer row), this is the exact `frame.area().height` `render::draw`
    /// itself sizes the messages pane against. `App::sync_view` threads this
    /// into `messages::sync` (mirroring how it already threads the active
    /// document's `viewport.width` through) so `messages::height` — the
    /// one function both `layout::geometry` and `sync` call — computes the
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
    /// own width (`messages::sync`, `layout::geometry` itself) reads
    /// this instead. `0` before the first `Msg::Resize`, exactly like
    /// `frame_height`. `App::relayout` is a no-op while this is `0`.
    pub frame_width: u16,
    /// The Explorer pane's own state: root, listing, cursor.
    /// Starts unloaded; `pane::handle_global_command` loads it on `^x`.
    pub explorer: Explorer,
    /// The Open Tabs pane's own state: tab display order and
    /// its cursor/scroll position — kept in sync with `documents` at its own
    /// chokepoints (`App::open_document`/`workspace::close_now`).
    pub tabs: OpenTabs,
    /// The Help virtual document's id, once minted — `None`
    /// until the first `F1`. Makes `workspace::toggle_help` idempotent (a
    /// second press never mints a duplicate).
    pub help_doc: Option<DocumentId>,
    /// The document active right before `F1` last activated Help —
    /// `workspace::toggle_help`'s target when toggling back off.
    pub help_return_to: Option<DocumentId>,
    /// The editable title field (`title.rs`) — the file's FULL name,
    /// extension included, that a rename types into. Reseeded at every
    /// document switch (`workspace::switch_to`) and at every focus gain
    /// (`App::focus_title`), so it always describes the document actually
    /// showing. Unjournaled at the document level: its own in-memory
    /// undo history (⌘Z/⇧⌘Z) never reaches the recovery store.
    pub title: crate::title::TitleField,
    /// The rename workflow's state (`rename.rs`) — a typed machine rather
    /// than another ad hoc pending-boolean, because `[R]eplace` has a
    /// mid-sequence point past which it is not cancellable.
    pub rename: crate::rename::RenameState,
    /// `pub(crate)`: `rename.rs` is the sole minter of new generations for
    /// its own `Cmd` route (mirrors `next_quit_gen`/`next_save_confirm_gen`).
    pub(crate) next_rename_gen: u32,
    /// Merge mode's own state machine — a plain field, like
    /// `rename` above. `merge::begin`/`merge::exit_in_place` are its
    /// writers.
    pub merge: crate::merge::MergeState,
    /// `pub(crate)`: `merge::begin` is the sole minter of new generations,
    /// mirroring `next_rename_gen` above.
    pub(crate) next_merge_gen: u32,
    /// This session's recovery store — this is the app-wide half,
    /// shared by every open document; each document's own binding lives
    /// in its `Document::db: Option<DocDb>`. `None` only when no store
    /// could be constructed at
    /// all (an extreme fallback distinct from `Db::degraded`, which still
    /// has a live, if untrusted, store).
    pub db: Option<Db>,
    /// Correlates an in-flight `rune-db` op id to the `DocumentId` that
    /// enqueued it, plus — for a `Load` op — the issuing document's
    /// `buffer.version()` at the moment it was enqueued — inserted as
    /// one `PendingOp` at every successful `Store` enqueue
    /// (`db::append_edit`/`move_undo_pos`/`db::
    /// load_document`/`save::materialize_now`/`materialize_ack::handle_snapshot_due`),
    /// removed by `handle_db_event` once its ack lands. Needed because the
    /// writer thread's single FIFO ack stream has no per-document identity
    /// of its own once more than one document can enqueue. `Load` is
    /// asynchronous, so the user may type into the buffer during the round
    /// trip; comparing the recorded version against the buffer's version AT
    /// ACK TIME is how `db::handle_load_ack` decides whether adopting the
    /// ack's recovered content would silently clobber those keystrokes
    /// (never clobber keystrokes to complete a recovery binding). Carrying
    /// both facts in one value, rather than two maps keyed by the same op
    /// id, means a sweep can never drop one and keep the other.
    pub db_ops: HashMap<u64, crate::db::PendingOp>,
    /// Correlates an in-flight `MaterializeRecord` op id to the
    /// document whose disk write ALREADY physically completed before this
    /// op was even enqueued — the caller-side vfs work runs first, this
    /// bookkeeping op runs after. A dead writer failing precisely THIS op
    /// (`DbEvent::Err`/`Fatal`) must never be reported as a failed save:
    /// `handle_db_event` consults this map to react with a
    /// synthetic committed ack instead of the ordinary failure path.
    /// Cleared on both success and failure — never left stale.
    pub published_ops: HashMap<u64, DocumentId>,
    /// The content/path/CAS facts a `materialize` attempt captured at
    /// trigger time, held here between `MaterializePrepare`'s ack (which
    /// carries no disk-sourced data at all) and the caller-side `vfs` `Cmd`
    /// it spawns — `save::PendingMaterialize`'s doc comment explains why
    /// each field is captured once and never re-derived.
    pub(crate) pending_materialize: HashMap<DocumentId, crate::save::PendingMaterialize>,
    /// A persistent status banner independent of the message log — set once
    /// the store degrades (at open, or from a later `on_store_failure`) and
    /// never cleared automatically.
    pub db_banner: Option<String>,
    /// The armed degraded-save confirm chord's target document and timer
    /// generation — `None` when no confirm is pending. Doc-tagged so a tab
    /// switch can't misapply an armed confirm gate; mirrors `pending_quit`
    /// below. Stale `SaveConfirmTimeout` generations are ignored.
    pub pending_save_confirm: Option<(DocumentId, u32)>,
    /// `pub(crate)`, not private: `save::trigger_save` — a different module
    /// — is the sole minter of new generations.
    pub(crate) next_save_confirm_gen: u32,
    /// The document a Guard modal's `[S]ave` armed a save-then-close for
    /// — `None` when no close is waiting on a save ack.
    /// `guard::handle_guard_key`'s Guard arm sets this immediately before
    /// calling `save::trigger_save`; `materialize_ack::handle_materialize_ack`/
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
    /// `pub(crate)`: `pane::handle_quit_key` (moved out of this module)
    /// is the sole minter of new generations.
    pub(crate) next_quit_gen: u32,
    /// The quit-save fan-out a `GuardKind::DirtyQuit` answer of `[S]ave`
    /// armed, correlating every document it kicked a save off
    /// for against the EXACT buffer version that save captured. A
    /// `BTreeMap`, not a bare counter: retiring one document's entry is
    /// idempotent (a duplicate/late ack can never double-decrement a
    /// counter that no longer exists as such), an unrelated ⌘S ack for a
    /// document quit never asked to save can't retire an entry it was
    /// never keyed to, and a document that disappears mid-flight (closed,
    /// or the whole store dying) is swept by removing its one entry rather
    /// than needing to keep a parallel count in sync. `None` whenever no
    /// quit-save fan-out is outstanding — the ordinary two-press quit-
    /// confirm chord never touches this at all. `materialize_ack`'s
    /// `quit_if_pending`/`retire_quit_wait` are the only writers of the map
    /// once armed; `should_quit` flips only when the LAST entry retires
    /// successfully.
    pub quit_intent: Option<QuitIntent>,
    /// The click-aggregation + drag-selection state a mouse gesture needs
    /// across messages — `commands::mouse`'s sole owner.
    pub pointer: crate::pointer::PointerState,
    /// Answers "what time is it right now?" for `pointer`'s multi-click
    /// window: the clock is a field so the fuzzer can inject time to
    /// reproduce a gesture. Production installs the real wall clock, tests
    /// install a `ManualClock`.
    pub pointer_clock: Box<dyn crate::pointer::Clock>,
    /// Which binding set governs the editor pane — defaults to
    /// `BindingSet::Default` (the VS Code-style set this crate has always
    /// had). `app::handle_editor_key` does not consult this yet: full vim
    /// modal editing is out of scope here (see `keymap::vim`'s doc
    /// comment); this field exists so a future dispatch switch has
    /// somewhere to read from.
    pub binding_set: crate::keymap::BindingSet,
    /// The single close/quit/rename/disk-conflict confirmation prompt —
    /// replaces the earlier `banner::Modal`'s `Guard` variant, whose
    /// `Error` sibling now lives in `messages` instead. `guard::set_guard`
    /// is the one chokepoint that writes a NEW prompt here; `guard::
    /// clear_guard` is the sole writer of `None`.
    pub guard: Option<GuardPrompt>,
    /// The message log: every transient user-facing message, severity-tagged,
    /// plus the collapsible pane's own open/focus state.
    /// `messages::post` (and its `info`/`warn`/`error` wrappers) is the one
    /// chokepoint that writes to it.
    pub messages: MessageLog,
    /// The in-file search bar's state — `None` when the bar is closed
    /// (decision: bar-open IS `search.is_some()`). `pub(crate)`: every
    /// writer (`search::open`/`close`/`recompute`, `search::keys::
    /// handle_key`) lives inside the `search` module; outside callers
    /// (`layout::geometry`, `render`) only ever read it.
    pub(crate) search: Option<crate::search::SearchState>,
    /// The last query the search bar held while open, kept after closing
    /// so a closed-bar next/prev chord (a later change) has something to
    /// navigate with. `None` until the bar has closed at least once with a
    /// non-empty query.
    pub(crate) last_search_query: Option<String>,
    /// The query most recently enqueued as a `TouchSearchQuery` write
    /// (`search::keys::persist_query`'s own debounce key) — lives on `App`,
    /// not `SearchState`, since a closed-bar chord (`advance_closed`) calls
    /// the same persist path with no `SearchState` to hold it. Repeated
    /// Enter on an unchanged query (e.g. wrapping back to the same single
    /// match) must not re-enqueue a write on every keystroke.
    pub(crate) last_persisted_search_query: Option<String>,
    /// The next generation `search::open` mints for a history load request
    /// — a plain counter on `App`, not on `SearchState`
    /// itself, because it must keep distinguishing requests across a
    /// close-then-reopen: a fresh `SearchState` starts every field over,
    /// but a reply to the PREVIOUS open's now-abandoned request must still
    /// be recognizable as stale rather than accidentally matching whatever
    /// generation the new state happens to start at. Mirrors
    /// `next_rename_gen`/`next_quit_gen`'s own shape.
    pub(crate) next_search_history_gen: u64,
    /// Op ids of an in-flight `TouchSearchQuery` write: a completed op id
    /// absent from `db_ops` still reaches `db_dispatch::handle_db_event`'s
    /// `DbEvent::Err` arm, which otherwise treats every unmatched failure
    /// as a real recovery failure and sticky-degrades the whole store.
    /// Tracking this cosmetic
    /// write's own op id here lets that arm recognize it and report a
    /// message instead of degrading. Cleared on both the write's success
    /// and its failure — never left stale.
    pub(crate) search_history_ops: HashSet<u64>,
    pub should_quit: bool,
    /// The rendered theme — the one `Theme` every chrome
    /// style and every markdown/code `ScopeId` in this app resolves
    /// through; nothing in `render.rs`/`explorer.rs`/`footer.rs`/etc. reads
    /// a raw indexed- or truecolor literal directly (those live only under
    /// `theme/`). `App::new` defaults to truecolor (Catppuccin Mocha,
    /// unquantized) — production
    /// startup (`rune-cli`) overwrites it once `term::Guard` exists and
    /// `theme::probe::supports_truecolor` can actually query the real
    /// terminal; every test and the fuzzer keep this default.
    pub theme: crate::theme::Theme,
    /// The icon tier — `theme::icons::choose`'s one decision,
    /// made once at startup from the real environment and held here beside
    /// `theme` for the same reason: nothing downstream re-decides it per
    /// frame. `App::new` defaults to the plain-Unicode tier (the same
    /// terminal-agnostic default `DocMachine::new` itself starts with);
    /// production startup (`runtime::bootstrap`) overwrites it once the
    /// real `TERM`/`TERM_PROGRAM`/`RUNE_ICONS` environment can be read —
    /// every test and the fuzzer keep this default.
    pub icons: rune_md::icons::IconSet,
    /// This process's Kitty graphics support and measured cell pixel
    /// geometry — populated at startup (`runtime::bootstrap`,
    /// alongside `theme` and `icons` above, for the same "decided once,
    /// never per frame" reason) and re-derived on every `Msg::Resize`
    /// (`runtime::apply`), since a resize can change the reported pixel
    /// dimensions even when the Kitty/truecolor decision itself cannot.
    /// `App::new` defaults to no-Kitty + `rune_image::DEFAULT_CELL_SIZE`
    /// (`graphics::GraphicsCaps::default`), so every existing test
    /// constructor and the fuzzer keep this default unchanged.
    pub graphics: crate::graphics::GraphicsCaps,
    /// The workspace root discovered by `workspaceroot::resolve` — the
    /// nearest ancestor of the launch directory carrying a
    /// `.git`/`.obsidian` marker, or `cwd` itself when none is found.
    /// `PathBuf::new()` (empty) until `set_root` runs; an empty root is a
    /// legal "not yet resolved" state, and every consumer (the Explorer's
    /// initial-root fallback, the breadcrumb's relativization) skips it
    /// rather than treating it as a real path.
    pub root: PathBuf,
    /// The snapshot-autosave debounce's one rearmable timer
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
            splits: crate::layout::Splits::default(),
            frame_height: 0,
            frame_width: 0,
            explorer: Explorer::default(),
            tabs: OpenTabs::new(id),
            help_doc: None,
            help_return_to: None,
            title: crate::title::TitleField::default(),
            rename: crate::rename::RenameState::default(),
            next_rename_gen: 0,
            merge: crate::merge::MergeState::default(),
            next_merge_gen: 0,
            db,
            db_ops: HashMap::new(),
            published_ops: HashMap::new(),
            pending_materialize: HashMap::new(),
            db_banner: None,
            pending_save_confirm: None,
            next_save_confirm_gen: 0,
            pending_close_on_save: None,
            pending_quit: None,
            next_quit_gen: 0,
            quit_intent: None,
            pointer: crate::pointer::PointerState::default(),
            pointer_clock: Box::new(crate::pointer::SystemClock),
            binding_set: crate::keymap::BindingSet::default(),
            guard: None,
            messages: MessageLog::new(),
            search: None,
            last_search_query: None,
            last_persisted_search_query: None,
            next_search_history_gen: 0,
            search_history_ops: HashSet::new(),
            should_quit: false,
            theme: crate::theme::Theme::catppuccin_mocha(false),
            icons: rune_md::icons::IconSet::unicode(),
            graphics: crate::graphics::GraphicsCaps::default(),
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
    /// Takes `db` rather than hard-coding `None`: the untitled draft is
    /// really recovery-backed, so the caller
    /// (`rune-cli::db_bootstrap`) opens the recovery store and binds this
    /// document's own scratch row (`Store::create_scratch`/
    /// `recoverable_scratch`) BEFORE constructing `App`, exactly like a named
    /// launch's `App::new` already receives its `Db` at construction. A
    /// caller with no store available (degraded open, `$HOME` unset) still
    /// passes `None` here — this document then behaves exactly as it always
    /// has, with no recovery journal for this launch.
    ///
    /// Also shows the left column: launched with no file to edit, the user
    /// needs somewhere to navigate from, so the Explorer/Open Tabs pane
    /// starts visible. A launch that names a file goes through `App::new`
    /// directly and keeps the default hidden left column instead, so the
    /// editor gets the full width for the document the user asked for.
    pub fn new_untitled(vfs: Arc<dyn Vfs + Send + Sync>, db: Option<Db>) -> App {
        let mut app = App::new(Buffer::new(""), None, vfs, db);
        app.active_doc_mut().display_name = Some("Untitled 1".to_string());
        app.splits.left.show();
        app
    }

    /// Mints the next `DocumentId`, monotonically — never reused, even
    /// across a close, though the counter design already supports it.
    /// `saturating_add` rather than `wrapping_add`: wrapping
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
    /// id. The minimal multi-document seam (its own unit tests in
    /// `db.rs`/`save.rs`, "two docs enqueue ops") — full open/close/switch
    /// UX is handled by `workspace::open_path`.
    pub fn open_document(&mut self, buffer: Buffer) -> DocumentId {
        let id = self.mint_doc_id();
        self.documents.insert(id, Document::new(buffer));
        // The Open Tabs chokepoint: every document, however
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

    /// Infallible by construction, not by convention: `DocumentMap`
    /// guarantees at least one entry always exists, so "`active`
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

    /// Convenience delegate to the active document's dirty cache — kept on
    /// `App` so render/status call sites don't need to spell out
    /// `app.active_doc().is_dirty()` everywhere.
    pub fn is_dirty(&self) -> bool {
        self.active_doc().is_dirty()
    }

    /// Whether `doc` has a live, trustworthy recovery journal — the single
    /// predicate `pane::unpreserved_dirty_docs`'s quit/close guard scan
    /// keys off. `doc.db.is_some()` alone is not enough: `degraded`
    /// is sticky with no reopen path, so a mid-session store failure leaves
    /// `doc.db` `Some` while every write it would enqueue silently never
    /// lands — a document in that state has stopped journaling in every
    /// way that matters, exactly as if it had no binding at all. `db: None`
    /// (no store at all) also fails this, same as before.
    pub fn is_preserved(&self, doc: &Document) -> bool {
        doc.db.is_some() && self.db.as_ref().is_some_and(|db| !db.degraded)
    }

    pub fn file_name(&self) -> &str {
        self.active_doc().file_name()
    }

    /// The public entry point `rune-cli`'s bootstrap hydration uses —
    /// `materialize_ack::recompute_dirty` itself is `pub(crate)`, so
    /// this is the one seam a different crate reaches it through.
    /// Dirty must be re-derived on every transition, hydration included;
    /// dirtiness no longer falls out of
    /// `Document::hydrate` itself (the deleted `mark_dirty_from_hydration`
    /// this replaces) since it is now a content comparison the caller must
    /// explicitly re-run once the buffer settles.
    pub fn recompute_dirty(&mut self, id: DocumentId) {
        crate::materialize_ack::recompute_dirty(self, id);
    }

    /// Records the workspace root discovered by `workspaceroot::resolve`
    /// — the one writer of `root`, called once at startup once
    /// the launch-time `cwd`/`home`/file-argument inputs the resolver needs
    /// are available.
    pub fn set_root(&mut self, root: PathBuf) {
        self.root = root;
    }

    // `focus.rs` is the sole writer of `focus` outside this constructor.
}

/// The ONLY writer of `App` state. `effects` accumulates I/O for the
/// runtime loop to perform after the whole message batch is applied:
/// `effects.raw` for OSC 52 (drained by the main loop, never a `Cmd` — plan
/// Gotchas, "Cmds must never touch the terminal"), `effects.cmds` for
/// off-thread work (save, pbpaste, the quit-confirm/save-confirm/snapshot
/// timers).
///
/// Wraps `update_inner` with the ONE chokepoint for the snapshot-autosave
/// debounce: every message that mutates the ACTIVE document's
/// `journal` — typing, undo/redo, cut, paste, ... — funnels through
/// `commands::edit::commit_edit_batch`/`undo`/`redo`, so comparing the
/// active document's journal position before and after `update_inner`
/// catches all of them uniformly, without threading a debounce call through
/// every editing command's call site individually.
pub fn update(app: &mut App, msg: Msg, effects: &mut Effects) {
    let journal_pos_before = app.active_doc().journal.pos();
    let active_before = app.active;
    let buffer_version_before = app.active_doc().buffer.version();
    let focus_before = app.focus();
    dispatch::update_inner(app, msg, effects);
    if app.active_doc().journal.pos() != journal_pos_before {
        let id = app.active;
        save::schedule_snapshot_debounce(app, id);
    }
    // The rest of the post-dispatch chokepoint (highlight scheduling, a
    // newly-active image document's decode, the embed reconciler) lives
    // in `dispatch::after_update` — split out so this file, already over
    // the 500-line budget, never needs to grow for a future addition to
    // that list.
    dispatch::after_update(app, active_before, buffer_version_before, effects);
    // The Explorer live preview's promote/discard-on-focus-move reaction
    // (`explorer_preview::on_focus_changed`) has no hook in `dispatch.rs`
    // itself: a pure focus transition (Escape from the Explorer landing on
    // the Editor, some other pane grabbing focus with no document switch of
    // its own) touches no document, so nothing in `workspace::switch_to`
    // ever runs for it. Comparing focus before and after the whole message
    // batch, here, is this crate's other before/after diff
    // (`active_before`/`buffer_version_before` above) doing the same job
    // for a different pair of fields.
    crate::explorer_preview::on_focus_changed(app, focus_before, app.focus());
}

// `relayout`/`sync_view` moved to `app_view.rs` (500-line budget) —
// both are still plain `impl App` methods, reached the same way as before.

// `schedule_highlight` moved to `highlight.rs` (500-line budget) —
// `update` above calls it through `highlight::`.

// `update_inner` (the top-level `Msg` dispatch), `handle_key`,
// `handle_editor_key` and `handle_db_event` moved to `dispatch.rs`
// (500-line budget) — `update` above calls the first through
// `dispatch::`.

// `handle_quit_key` and its 2s timer `Cmd` moved to `pane.rs`
// (500-line budget) — `GlobalCommand::QuitChord` is its only
// remaining caller. Their
// unit tests moved to `tests/app_quit_and_dispatch.rs`.
