//! The Elm-style runtime: `Msg`, `Cmd`, `Effects`, and the main loop.
//! This module's `run` (main: recv -> drain
//! `try_iter` -> `update` per message -> drain `Effects.out` to the terminal
//! -> spawn `Effects.cmds` -> draw once), the input reader spawned by `run`,
//! one `std::thread` per `Cmd`, and `App::timers`'s own single
//! long-lived rearmable timer thread — the one background
//! thread NOT spawned fresh per `Cmd`, since (re)arming any of its four
//! keyed deadlines is a plain state update rather than new off-thread work.
//!
//! `update` mutates `App` synchronously — synchronous state changes directly
//! in `update`; a Cmd is exclusively for I/O that leaves the thread.
//! `Effects.out` is the ONLY path by which escape bytes
//! (OSC 52 clipboard writes) reach the terminal — a `Cmd` never touches it;
//! termina's `Terminal`
//! is `io::Write` on `&mut self`, single-owner, undocumented for cross-
//! thread use.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use rune_vfs::{DirEntry, Vfs};

use crate::document::DocumentId;
use crate::highlight::PassOutcome;
use crate::keymap::KeyInput;
use crate::pointer::MouseInput;

/// Where a `Msg::ClipboardRead`'s text is destined. Captured when the
/// `pbpaste` `Cmd` is spawned (`clipboard::pbpaste_cmd`), never resolved
/// from live focus/active-document state when the reply arrives — that is
/// what makes a document switch or a focus change mid-flight unable to
/// redirect the paste. `Msg::Paste` (bracketed paste from the terminal) has
/// no request to attach a target to, so it keeps routing by live focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteTarget {
    Document(DocumentId),
    /// The title field, tagged with the document it was seeded for.
    /// A reply whose document is no longer the active one is dropped: the
    /// title is reseeded at every focus gain and document switch, so an
    /// untagged reply could land one title session's clipboard text into a
    /// different document's name.
    Title(DocumentId),
    /// The search bar's draft. Untagged (the bar is a single global field,
    /// not per-document like the title): `search::keys::paste` drops the
    /// reply on arrival if the bar has since closed, the same "reply lands
    /// only where it can still make sense" discipline `Title`'s own tag
    /// enforces by a different mechanism.
    Search,
}

/// A typed error crossing a `Cmd` boundary — the reply side of every
/// off-thread call this module spawns that can fail. Preserves the
/// underlying source's kind (an `io::ErrorKind` from a `Vfs` call, a
/// `rune_vfs::GetRefusal`, a `rune_db::Error`, a `rune_image::ImageError`)
/// rather than flattening it to a message at the `Cmd` closure itself, so a
/// handler CAN branch on what actually failed; `Refused` is the escape
/// hatch for a business refusal that never had a typed source to begin
/// with (an unexpected reply shape, a redundant publish outcome). Every
/// variant's `Display` renders exactly the text the flattened `String` used
/// to carry, so message-pane wording is unchanged.
#[derive(Debug)]
pub enum CmdError {
    Io(io::Error),
    Get(rune_vfs::GetRefusal),
    Db(rune_db::Error),
    Image(rune_image::ImageError),
    Refused(String),
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CmdError::Io(e) => write!(f, "{e}"),
            CmdError::Get(e) => write!(f, "{e}"),
            CmdError::Db(e) => write!(f, "{e}"),
            CmdError::Image(e) => write!(f, "{e}"),
            CmdError::Refused(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CmdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CmdError::Io(e) => Some(e),
            CmdError::Get(e) => Some(e),
            CmdError::Db(e) => Some(e),
            CmdError::Image(e) => Some(e),
            CmdError::Refused(_) => None,
        }
    }
}

impl From<io::Error> for CmdError {
    fn from(e: io::Error) -> Self {
        CmdError::Io(e)
    }
}

impl From<rune_vfs::GetRefusal> for CmdError {
    fn from(e: rune_vfs::GetRefusal) -> Self {
        CmdError::Get(e)
    }
}

impl From<rune_db::Error> for CmdError {
    fn from(e: rune_db::Error) -> Self {
        CmdError::Db(e)
    }
}

impl From<rune_image::ImageError> for CmdError {
    fn from(e: rune_image::ImageError) -> Self {
        CmdError::Image(e)
    }
}

/// One runtime event. `Key`/`Paste`/`Resize`/`Mouse` originate from the
/// input-reader thread; `ClipboardRead`/`SaveDone` originate from a spawned
/// `Cmd`'s return value; `Timer`/`SnapshotDue` all originate from
/// `App::timers`'s one long-lived rearmable timer thread, not a per-message
/// spawned `Cmd`; `Db` originates from the `rune-db` writer thread via
/// `db::DbBridge`; `Error`/`Quit` can be synthesized by `update` itself.
/// `SaveDone`/`SnapshotDue` carry a `DocumentId` so multi-
/// document acks route back to the document that triggered them; `Timer`
/// stays doc-agnostic — `App::quit` is app-wide, `pending_save_confirm`'s
/// doc tag lives in the `Option` tuple itself, and the message log is a
/// single app-wide pane — none of them need a `Msg`-carried document
/// identity.
#[derive(Debug)]
pub enum Msg {
    Key(KeyInput),
    PumpGraphics,
    Paste(String),
    Resize(u16, u16),
    /// A mouse event, translated from `termina::Event::Mouse` —
    /// `commands::mouse::handle` is its sole handler.
    Mouse(MouseInput),
    /// A `pbpaste` reply. `target` is captured when the `Cmd` is spawned
    /// (`clipboard::pbpaste_cmd`), not resolved on arrival — a document
    /// switch or a focus change while the subprocess is in flight cannot
    /// redirect the paste to whatever happens to be focused/active by the
    /// time the reply lands.
    ClipboardRead {
        text: String,
        target: PasteTarget,
    },
    SaveDone {
        id: DocumentId,
        ticket: crate::document::SaveTicket,
        version: u64,
        result: Result<(), CmdError>,
        durable: bool,
    },
    /// The quit-confirm window, the degraded-save confirm gate, and the
    /// message pane's auto-collapse timer — `TimerKey::{QuitConfirm,
    /// SaveConfirm, MessagesCollapse}` — all share this one stale-
    /// generation-is-ignored shape, dispatched by matching on `key`.
    Timer {
        key: crate::runtime::TimerKey,
        generation: u64,
    },
    /// The 2s snapshot-autosave debounce timer — a stale
    /// generation (a later journal mutation already rescheduled) is
    /// ignored.
    SnapshotDue {
        id: DocumentId,
        generation: u32,
    },
    /// A completion posted by `rune-db`'s writer thread, routed through
    /// `db::DbBridge`.
    Db(rune_db::DbEvent),
    /// The caller-side `vfs` `Cmd` a `MaterializePrepare` ack spawned
    /// (`save::materialize_vfs_cmd`) has finished the ENTIRE disk dance —
    /// resolve/read/hash-compare/publish/read-displaced — through this
    /// app's own `Vfs` handle, never the writer thread's. Routed to
    /// `materialize_ack::handle_materialize_vfs_done`.
    MaterializeVfsDone {
        id: DocumentId,
        ticket: crate::document::SaveTicket,
        db_id: i64,
        seq: i64,
        content: std::sync::Arc<str>,
        outcome: crate::materialize_ack::MaterializeVfsOutcome,
    },
    /// `vfs.read_dir(root)` completed — the Explorer's own
    /// boundary Msg, delivered by [`load_dir_cmd`]. `Nav` (navigated into
    /// `root`) resets the Explorer's cursor to the top; `Refresh` (a future
    /// watcher-triggered reload, out of scope for now — no
    /// production caller constructs it yet) preserves the selected entry by
    /// name when it's still present. A `read_dir` failure becomes
    /// `Msg::Error` instead — see `load_dir_cmd`. `generation` is the
    /// request's own generation token (review fix: two in-flight `ReadDir`
    /// Cmds can land out of order) — `explorer::handle_dir_loaded` ignores a
    /// reply whose `generation` no longer matches `Explorer::request_
    /// generation`.
    DirLoaded {
        root: PathBuf,
        entries: Vec<DirEntry>,
        cause: DirCause,
        generation: crate::generation::DirLoadGen,
    },
    /// A rename/draft-create `Cmd` completed (the no-store route). Carries
    /// its own `generation` so a reply to a rename the user has since
    /// dismissed and restarted is dropped rather than applied to the fresh
    /// one — `spawn_cmd` has no cancellation, so this echo is the only
    /// thing standing between a late reply and a corrupted state.
    RenameDone {
        generation: crate::generation::RenameGen,
        result: Result<rune_db::RenameOutcome, CmdError>,
    },
    /// A `Trash` `Cmd` completed — `trash::confirm`'s reply, routed to
    /// `trash::handle_trash_done`. Carries its own `generation` so a reply
    /// to a trash the user has since dismissed (there is no dismiss path
    /// once confirmed, but a fresh trash request can still overwrite
    /// `App::trash_gen` before this one lands) is dropped rather than
    /// applied to the fresh one.
    TrashDone {
        generation: crate::generation::TrashGen,
        path: PathBuf,
        result: Result<(), CmdError>,
    },
    /// A `ReadFile` `Cmd` completed —
    /// `workspace::open_path_async`'s reply, routed to `workspace::
    /// handle_file_opened`. `anchor` is carried through unchanged from the
    /// request so landing it (`navigate::land_anchor`) doesn't need a
    /// second round trip once the document is open. No `generation`/
    /// staleness echo: unlike rename/save, opening a file mutates no
    /// shared single-slot machine state — `handle_file_opened` rechecks
    /// `existing_document_for` itself, so two overlapping opens of the
    /// same path just converge on one document rather than racing.
    FileOpened {
        path: PathBuf,
        result: Result<Vec<u8>, CmdError>,
        anchor: Option<rune_nav::Anchor>,
    },
    /// A background highlight call completed. `result` says what the pass
    /// came back with in its own words — its variants' docs carry the
    /// keep-or-replace semantics. `version` is the buffer version the
    /// highlight ran against; a reply whose `version` no longer matches the
    /// live buffer is dropped whole.
    Highlighted {
        doc: DocumentId,
        version: u64,
        result: PassOutcome,
    },
    /// The deferred bootstrap display-pipeline compute for a large document
    /// completed — `bootstrap::bootstrap_view_cmd`'s reply, routed to
    /// `dispatch::handle_bootstrap_view_ready`. `version` is the buffer
    /// version the compute ran against; a reply whose `version` no longer
    /// matches the live buffer (an edit landed during the wait) is dropped —
    /// the next ordinary `App::sync_view` recomputes from the live buffer
    /// instead, the same fallback every stale-reply case in this enum takes.
    BootstrapViewReady {
        id: DocumentId,
        version: u64,
        machine: Box<rune_md::element::doc::DocMachine>,
        view: rune_md::element::doc::ViewSnapshots,
    },
    /// An image document's background decode completed,
    /// routed to `graphics::handle_image_decoded`. `generation` echoes
    /// `ImageState::in_flight` — `spawn_cmd` has no cancellation, so a
    /// reply whose generation no longer matches is dropped silently.
    ImageDecoded {
        doc: DocumentId,
        generation: crate::generation::ImageDecodeGen,
        result: Result<rune_image::decode::Decoded, CmdError>,
    },
    EmbedDecoded {
        doc: DocumentId,
        generation: u64,
        result: Result<rune_image::decode::Decoded, CmdError>,
    },
    Error(String),
    /// The same transport as `Error`, tagged one severity down: a
    /// background task hit something worth telling the user
    /// about, but not something as severe as `Error`'s glyph/persistence
    /// implies.
    Warning(String),
    /// One recents/MRU load reply — the search bar's history
    /// ([`load_search_history_cmd`], routed to `search::
    /// handle_history_loaded`), the fuzzy file finder's recents
    /// ([`filesearch_recents_cmd::load_filesearch_recents_cmd`], routed to
    /// `filesearch::handle_recents_loaded`), and the command palette's
    /// recent commands (`command_history_cmd::load_command_history_cmd`,
    /// routed to `palette::handle_recents_loaded`) — collapsed onto one
    /// shape, `kind` selecting the handler. `generation` echoes the
    /// requesting state's own counter, erased to its raw form the same way
    /// `Msg::Timer`'s does — each handler reconstructs its own typed
    /// `Generation<T>` via `from_raw` before comparing, so a search-history
    /// generation can never be compared against a palette one by mistake.
    /// A reader `Err` still carries this variant rather than folding into
    /// `Msg::Error`/`Msg::Warning`, so a stale reply is discarded exactly
    /// like a fresh one instead of always surfacing a message regardless of
    /// generation.
    RecentsLoaded {
        kind: RecentsKind,
        generation: u64,
        result: RecentsResult,
    },
    /// The fuzzy file finder's ignore-aware workspace walk completed —
    /// `filesearch::open`'s own `Cmd`, delivered by [`filesearch_cmd::
    /// filesearch_scan_cmd`] and routed to `filesearch::handle_scanned`.
    /// `generation` echoes `FileSearchState::generation`; a reply whose
    /// generation no longer matches (the finder closed and reopened since
    /// this scan was issued) is dropped, the same shape `Msg::DirLoaded`
    /// uses. A scan failure (the resolved root vanished, or resolved to
    /// something other than a directory) rides the same `Err` channel
    /// rather than `Msg::Error`/`Msg::Warning` directly, so a stale failure
    /// is discarded exactly like a stale success instead of always
    /// surfacing a message nobody's still waiting on.
    FileSearchScanned {
        generation: crate::generation::FileSearchGen,
        result: Result<crate::filesearch::walk::ScanResult, String>,
    },
    Quit,
}

/// Which recents/MRU load a [`Msg::RecentsLoaded`] reply answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecentsKind {
    Search,
    FileSearch,
    Palette,
}

/// A [`Msg::RecentsLoaded`] reply's payload — `Search`/`Palette` share the
/// plain MRU-string-list shape; `FileSearch`'s recents are file candidates
/// carrying their own path/metadata, a genuinely different shape rather
/// than a coincidental duplicate, so it gets its own case instead of being
/// forced into `Strings`.
#[derive(Debug)]
pub enum RecentsResult {
    Strings(Result<Vec<String>, CmdError>),
    Candidates(Result<Vec<crate::filesearch::Candidate>, CmdError>),
}

/// Why a `Msg::DirLoaded` was requested — `explorer::
/// handle_dir_loaded` reacts differently: `Nav` (the user navigated to a
/// new root — Enter on a directory, Backspace to the parent, or the initial
/// `^x` load) resets the cursor to the top; `Refresh` (reserved for a later
/// fsnotify-driven reload — out of scope here) preserves the currently
/// selected entry by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirCause {
    Nav,
    Refresh,
}

mod cmd;
pub use cmd::{Cmd, CmdKind};

mod effects;
pub use effects::{Effects, Outbound};
use effects::{Sink, discharge};

mod run_loop;
pub use run_loop::run;
use run_loop::{apply, spawn_cmd, spawn_input_reader};

/// Reads `root`'s children off-thread via `vfs.read_dir` and
/// replies with `Msg::DirLoaded`, or `Msg::Error` on a read failure — the
/// Explorer's own boundary Msg, called from `explorer_keys::handle_key` (Open on
/// a directory, Backspace to the parent) and from `pane::handle_global_
/// command`'s `FocusExplorer` arm (the very first load). The
/// filesystem is reached only through the injected `Vfs`; this I/O
/// never runs inline in `update`, only inside a spawned `Cmd`. `generation`
/// is echoed back verbatim on the `Msg::DirLoaded` reply — every call site
/// passes `Explorer::request_generation` AFTER bumping it, so a later
/// request's reply can never be shadowed by an earlier, slower one landing
/// after it (review fix).
pub fn load_dir_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    root: PathBuf,
    cause: DirCause,
    generation: crate::generation::DirLoadGen,
) -> Cmd {
    Cmd::read_dir(move || match vfs.read_dir(&root) {
        Ok(entries) => Some(Msg::DirLoaded {
            root,
            entries,
            cause,
            generation,
        }),
        Err(e) => Some(Msg::Warning(format!(
            "could not list {}: {e}",
            root.display()
        ))),
    })
}

/// Reads `path` off-thread via `rune_vfs::get` —
/// `workspace::open_path_async`'s only `Cmd`, and `load_dir_cmd`'s single-
/// file counterpart. `anchor` is opaque data here, just carried through to
/// the `Msg::FileOpened` reply unchanged — this `Cmd` never resolves it
/// itself (that needs the target's own catalogue, which doesn't exist
/// until the document is open).
pub fn read_file_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    anchor: Option<rune_nav::Anchor>,
) -> Cmd {
    Cmd::read_file(move || {
        let result = rune_vfs::get(vfs.as_ref(), &path, Some(rune_vfs::MAX_DOCUMENT_BYTES))
            .map(|sighting| sighting.bytes)
            .map_err(CmdError::from);
        Some(Msg::FileOpened {
            path,
            result,
            anchor,
        })
    })
}

/// Loads the search bar's MRU history off-thread through a cloned
/// `ReaderQuery` — the reader thread's own connection, never
/// the writer's, so this can never contend with or block on an in-flight
/// recovery write. Always replies with `Msg::RecentsLoaded { kind:
/// RecentsKind::Search, .. }`, `generation` carried through unchanged: a
/// query failure becomes `result: Err(..)` rather than `Msg::Error`/
/// `Msg::Warning` directly, so `search::handle_history_loaded` can apply
/// the same stale-generation check to a failure as to a success instead of
/// always surfacing a message even for a reply nobody's still waiting on.
pub fn load_search_history_cmd(
    reader: rune_db::ReaderQuery,
    generation: crate::generation::SearchHistoryGen,
) -> Cmd {
    Cmd::search_history(move || {
        let result = reader
            .query(rune_db::ReaderRequestKind::RecentSearches { limit: 200 })
            .map(|reply| match reply {
                rune_db::ReaderReply::RecentSearches(entries) => entries,
                _ => Vec::new(),
            })
            .map_err(CmdError::from);
        Some(Msg::RecentsLoaded {
            kind: RecentsKind::Search,
            generation: generation.raw(),
            result: RecentsResult::Strings(result),
        })
    })
}

// The Explorer live-preview `Cmd` constructor — split out for the same
// 500-line-budget reason as `highlight_cmd`/`timer` below.
mod preview_cmd;
pub use preview_cmd::{MAX_PREVIEW_BYTES, read_preview_cmd};

mod bootstrap;
mod exit_settle;

mod transmit_queue;
pub use transmit_queue::{Pumped, TransmitQueue};

// The tree-sitter highlight `Cmd` constructor and the region pass behind it
// moved to `runtime::highlight_cmd` (500-line budget) — re-exported below so
// every existing `runtime::` call site keeps working unchanged.
mod highlight_cmd;
#[cfg(test)]
pub(crate) use highlight_cmd::test_clock;
pub(crate) use highlight_cmd::{FIRST_PAINT_BUDGET, PassBudget, highlight_cmd, run_regions};
pub use highlight_cmd::{PARSE_BUDGET, PASS_BUDGET};

// The comrak reveal-emit reuse path a ```markdown fence highlights through
// — its own file since it pulls in `rune_md::parse`/`emit`, a dependency
// `highlight_cmd` itself has no other reason to carry. Reached only from
// `highlight_cmd::run_regions`, never re-exported.
mod md_fence;

// The one rearmable timer thread shared by the snapshot-autosave debounce,
// the degraded-save confirm gate, the quit-confirm window, and the message
// pane's auto-collapse — split out for the same reason `highlight_cmd` was:
// a distinct concern with its own `#[cfg(test)]` module, kept out of this
// file's own 500-line budget.
mod timer;
pub use timer::{TimerKey, TimerService};

// The fuzzy file finder's recents-load `Cmd` constructor — split out for
// the same reason `highlight_cmd`/`timer` were (500-line budget).
mod filesearch_recents_cmd;
pub use filesearch_recents_cmd::load_filesearch_recents_cmd;
// The fuzzy file finder's workspace-walk `Cmd` — split out for the same
// 500-line-budget reason; `filesearch::open` is its only caller, reached
// through `runtime::` like every other `Cmd` constructor in this file.
mod filesearch_cmd;
pub(crate) use filesearch_cmd::filesearch_scan_cmd;

mod command_history_cmd;
pub use command_history_cmd::load_command_history_cmd;
