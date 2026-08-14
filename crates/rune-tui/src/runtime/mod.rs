//! The Elm-style runtime: `Msg`, `Cmd`, `Effects`, and the main loop.
//! This module's `run` (main: recv -> drain
//! `try_iter` -> `update` per message -> drain `Effects.raw` to the terminal
//! -> spawn `Effects.cmds` -> draw once), the input reader spawned by `run`,
//! one `std::thread` per `Cmd`, and `App::snapshot_timer`'s own single
//! long-lived rearmable timer thread — the one background
//! thread NOT spawned fresh per `Cmd`, since re-arming it is a plain state
//! update rather than new off-thread work.
//!
//! `update` mutates `App` synchronously — synchronous state changes directly
//! in `update`; a Cmd is exclusively for I/O that leaves the thread.
//! `Effects.raw` is the ONLY path by which escape bytes
//! (OSC 52 clipboard writes) reach the terminal — a `Cmd` never touches it;
//! termina's `Terminal`
//! is `io::Write` on `&mut self`, single-owner, undocumented for cross-
//! thread use.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use rune_vfs::{DirEntry, Vfs};

use crate::app::{self, App};
use crate::document::DocumentId;
use crate::highlight::HighlightReply;
use crate::keymap::{self, KeyInput};
use crate::pointer::MouseInput;
use crate::term::Guard;

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

/// One runtime event. `Key`/`Paste`/`Resize`/`Mouse` originate from the
/// input-reader thread; `ClipboardRead`/`SaveDone`/`ConfirmTimeout`/
/// `SaveConfirmTimeout`/`MessagesCollapseTimeout` originate from a spawned
/// `Cmd`'s return value; `SnapshotDue` originates from `App::snapshot_timer`'s
/// one long-lived rearmable timer thread, not a per-message
/// spawned `Cmd`; `Db` originates from the `rune-db` writer thread via
/// `db::DbBridge`; `Error`/`Quit` can be synthesized by
/// `update` itself.
/// `SaveDone`/`SnapshotDue` carry a `DocumentId` so multi-
/// document acks route back to the document that triggered them;
/// `ConfirmTimeout`/`SaveConfirmTimeout`/`MessagesCollapseTimeout` stay
/// doc-agnostic — `App::quit` is app-wide, `pending_save_confirm`'s doc
/// tag lives in the `Option` tuple itself, and the message log is a single
/// app-wide pane — none of them need a `Msg`-carried document identity.
#[derive(Debug)]
pub enum Msg {
    Key(KeyInput),
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
        result: Result<(), String>,
        durable: bool,
    },
    ConfirmTimeout {
        generation: crate::generation::Generation,
    },
    /// The 2s degraded-save confirm-gate timer (mirroring
    /// `ConfirmTimeout`'s quit-confirm shape) — a stale generation is
    /// ignored exactly like `ConfirmTimeout`.
    SaveConfirmTimeout {
        generation: crate::generation::Generation,
    },
    /// The message pane's 5s auto-collapse timer, armed by
    /// `dispatch::after_update` rather than by `messages::post` itself —
    /// same stale-generation-is-ignored shape as `ConfirmTimeout`/
    /// `SaveConfirmTimeout`.
    MessagesCollapseTimeout {
        generation: u32,
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
        generation: u32,
    },
    /// A rename/draft-create `Cmd` completed (the no-store route). Carries
    /// its own `generation` so a reply to a rename the user has since
    /// dismissed and restarted is dropped rather than applied to the fresh
    /// one — `spawn_cmd` has no cancellation, so this echo is the only
    /// thing standing between a late reply and a corrupted state.
    RenameDone {
        generation: crate::generation::Generation,
        result: Result<rune_db::RenameOutcome, String>,
    },
    /// A `Trash` `Cmd` completed — `trash::confirm`'s reply, routed to
    /// `trash::handle_trash_done`. Carries its own `generation` so a reply
    /// to a trash the user has since dismissed (there is no dismiss path
    /// once confirmed, but a fresh trash request can still overwrite
    /// `App::trash_gen` before this one lands) is dropped rather than
    /// applied to the fresh one.
    TrashDone {
        generation: u32,
        path: PathBuf,
        result: Result<(), String>,
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
        result: Result<Vec<u8>, String>,
        anchor: Option<rune_nav::Anchor>,
    },
    /// A background highlight call completed. `result: None` means NO
    /// RESULT — not one region produced anything, because every parse
    /// budget elapsed or no language resolved — and is distinguishable from
    /// `Some(..)` carrying empty payloads, a real empty result: `None` must
    /// leave every region's stored tree/spans untouched, or a document whose
    /// parse is slower than the budget would lose its colours on every
    /// keystroke and never regain them. `version` is the buffer version the
    /// highlight ran against; a reply whose `version` no longer matches the
    /// live buffer is dropped the same way.
    Highlighted {
        doc: DocumentId,
        version: u64,
        result: Option<HighlightReply>,
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
        generation: u64,
        result: Result<rune_image::decode::Decoded, String>,
    },
    EmbedDecoded {
        doc: DocumentId,
        generation: u64,
        result: Result<rune_image::decode::Decoded, String>,
    },
    Error(String),
    /// The same transport as `Error`, tagged one severity down: a
    /// background task hit something worth telling the user
    /// about, but not something as severe as `Error`'s glyph/persistence
    /// implies.
    Warning(String),
    /// The search bar's MRU history load, requested once per bar-open
    /// (`search::open`) and delivered by [`load_search_history_cmd`].
    /// `generation` echoes the `SearchState::history_generation` minted at
    /// the request — `search::handle_history_loaded` drops a reply whose
    /// generation no longer matches (a close-then-reopen since issued it),
    /// the same shape `Msg::DirLoaded` uses. A reader `Err` still carries
    /// this variant (with an `Err` result) rather than folding into
    /// `Msg::Error`, so a stale reply is discarded exactly like a fresh one
    /// instead of always surfacing a message regardless of generation.
    SearchHistory {
        generation: crate::generation::Generation,
        result: Result<Vec<String>, String>,
    },
    /// The fuzzy file finder's recents load, requested once per finder-open
    /// (`filesearch::open`) and delivered by [`filesearch_recents_cmd::
    /// load_filesearch_recents_cmd`]. `generation` echoes `FileSearchState::
    /// generation` minted at the request — `filesearch::
    /// handle_recents_loaded` drops a reply whose generation no longer
    /// matches (a close-then-reopen since issued it), the same shape
    /// `Msg::SearchHistory` uses. A reader `Err` still carries this variant
    /// (with an `Err` result) rather than folding into `Msg::Error`, for the
    /// same reason `SearchHistory` does: a stale reply is discarded exactly
    /// like a fresh one instead of always surfacing a message regardless of
    /// generation.
    FileSearchRecentsLoaded {
        generation: crate::generation::Generation,
        result: Result<Vec<crate::filesearch::Candidate>, String>,
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
        generation: crate::generation::Generation,
        result: Result<crate::filesearch::walk::ScanResult, String>,
    },
    Quit,
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

/// What one `update` call asks the runtime to do. `raw` is escape-byte
/// output (OSC 52): the main loop drains it to the terminal writer with
/// `write_all` + `flush` AFTER the message batch and BEFORE the next draw —
/// same thread as `draw`, so raw output and frames are serialized by
/// construction. `cmds` are spawned one `std::thread` each after `raw` is
/// drained.
#[derive(Default)]
pub struct Effects {
    pub cmds: Vec<Cmd>,
    pub raw: Vec<Vec<u8>>,
    /// Forces `apply` to clear the terminal's diff buffer —
    /// ratatui only repaints changed cells, which would leave a stale
    /// placement on screen after a retransmit whose placeholder cells
    /// stayed byte-identical (see `graphics::resize_refit`'s own docs).
    pub force_redraw: bool,
}

/// Runs the editor until the user quits or the input stream ends. Owns the
/// terminal for the lifetime of this call: `term::Guard` wraps a
/// `termina::Terminal`, single-owner and main-thread-only by the crate's own
/// design (see module docs).
pub fn run(app: &mut App) -> io::Result<()> {
    let bootstrap::Bootstrap {
        mut guard,
        tx,
        rx,
        mut save_handles,
    } = bootstrap::bootstrap(app)?;

    // The normal exit is `app.should_quit` becoming true, set either by
    // `Msg::Quit` (quit-confirm) or synthesized by `spawn_input_reader`
    // itself when its `events.read` fails (input stream gone — tty closed,
    // SIGHUP, ...): it sends `Msg::Error` then `Msg::Quit` before exiting,
    // specifically so this loop is never left blocking on `rx.recv()`
    // forever while holding an unsaved buffer hostage (no recovery store in
    // Phase 1). The `while let` here is a total fallback for the case where
    // literally every `Sender` clone (the reader's and any in-flight
    // `Cmd`'s) has been dropped without sending anything — shouldn't happen
    // given the above, but keeps this loop correct even if it did.
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(msg) = rx.try_recv() {
            batch.push(msg);
        }

        for msg in batch {
            apply(app, msg, &mut guard, &tx, &mut save_handles)?;
        }

        if app.should_quit {
            break;
        }

        app.sync_view();
        guard.draw(|frame| crate::render::draw(app, frame))?;
    }

    // Every fallback save `Cmd` spawned above is joined before `run` returns
    // and `main` drains/shuts down the store: an in-flight one finishes its
    // atomic publish; an already-finished one joins immediately. Quit is
    // reported as complete only once this returns.
    for handle in save_handles.drain(..) {
        let _ = handle.join();
    }

    Ok(())
}

/// Runs `update` for one message and immediately discharges its `Effects` —
/// raw bytes to the terminal, `Cmd`s to their own thread. Shared by the
/// resize-seeding call above and the main loop so there is exactly one
/// "apply a message" chokepoint.
fn apply(
    app: &mut App,
    msg: Msg,
    guard: &mut Guard,
    tx: &mpsc::Sender<Msg>,
    save_handles: &mut Vec<thread::JoinHandle<()>>,
) -> io::Result<()> {
    // A resize can change the terminal's reported pixel
    // dimensions even when the Kitty/truecolor decision itself cannot, so
    // `app.graphics` is re-derived here — the one "apply a message"
    // chokepoint this module's own doc comment above describes — rather
    // than only once at `bootstrap` time.
    let is_resize = matches!(msg, Msg::Resize(_, _));
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
    discharge(&mut effects, guard, tx, save_handles)?;
    if is_resize {
        crate::graphics::redetect(app, guard);
    }
    Ok(())
}

fn discharge(
    effects: &mut Effects,
    guard: &mut Guard,
    tx: &mpsc::Sender<Msg>,
    save_handles: &mut Vec<thread::JoinHandle<()>>,
) -> io::Result<()> {
    for raw in effects.raw.drain(..) {
        guard.write_raw(&raw)?;
    }
    for cmd in effects.cmds.drain(..) {
        spawn_cmd(cmd, tx.clone(), save_handles);
    }
    if effects.force_redraw {
        guard.force_redraw();
    }
    Ok(())
}

/// A panicking `Cmd` must not vanish silently — `update` might be waiting on
/// exactly this `Cmd`'s reply with no other input in flight, which would
/// otherwise leave the main loop's `rx.recv()` blocked forever. Catching the
/// unwind here and reporting it as `Msg::Error` keeps that impossible: every
/// spawned `Cmd` thread sends SOMETHING back, success, `None`, or a caught
/// panic.
///
/// `CmdKind::Save`'s handle is retained in `save_handles` (pruning already-
/// finished ones first) so `run` can join it on quit instead of letting
/// `JoinHandle::drop` detach it — every other kind is fire-and-forget
/// exactly as before.
fn spawn_cmd(cmd: Cmd, tx: mpsc::Sender<Msg>, save_handles: &mut Vec<thread::JoinHandle<()>>) {
    let is_save = cmd.kind() == CmdKind::Save;
    let handle = thread::spawn(move || {
        // Both sends below discard a closed-channel failure the same way
        // `spawn_input_reader` does: `tx` only closes once the main loop
        // has exited, so there is nothing left to notify either way.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cmd.run())) {
            Ok(Some(msg)) => {
                let _ = tx.send(msg);
            }
            Ok(None) => {}
            Err(_) => {
                let _ = tx.send(Msg::Error("a background task panicked".to_string()));
            }
        }
    });
    if is_save {
        save_handles.retain(|h| !h.is_finished());
        save_handles.push(handle);
    }
}

fn spawn_input_reader(events: termina::EventReader, tx: mpsc::Sender<Msg>) {
    thread::spawn(move || {
        loop {
            match events.read(|_| true) {
                Ok(event) => {
                    if let Some(msg) = translate_event(event)
                        && tx.send(msg).is_err()
                    {
                        return; // main loop gone; nothing left to notify
                    }
                }
                Err(e) => {
                    // The input source is gone (tty closed, SIGHUP, the
                    // process losing its controlling terminal, ...) — see
                    // `run`'s doc comment on why this must not just exit
                    // silently.
                    let _ = tx.send(Msg::Error(format!("input stream ended: {e}")));
                    let _ = tx.send(Msg::Quit);
                    return;
                }
            }
        }
    });
}

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
    generation: u32,
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
            .map_err(|e| e.to_string());
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
/// recovery write. Always replies with `Msg::SearchHistory`, `generation`
/// carried through unchanged: a query failure becomes `result: Err(..)`
/// rather than `Msg::Error`/`Msg::Warning` directly, so `search::
/// handle_history_loaded` can apply the same stale-generation check to a
/// failure as to a success instead of always surfacing a message even for a
/// reply nobody's still waiting on.
pub fn load_search_history_cmd(
    reader: rune_db::ReaderQuery,
    generation: crate::generation::Generation,
) -> Cmd {
    Cmd::search_history(move || {
        let result = reader
            .query(rune_db::ReaderRequestKind::RecentSearches { limit: 200 })
            .map(|reply| match reply {
                rune_db::ReaderReply::RecentSearches(entries) => entries,
                _ => Vec::new(),
            })
            .map_err(|e| e.to_string());
        Some(Msg::SearchHistory { generation, result })
    })
}

/// The Explorer live-preview's largest previewable file, in bytes — past
/// this, `explorer_preview` skips the read entirely rather than pulling a
/// huge file into memory just because the cursor happened to pass over it.
pub const MAX_PREVIEW_BYTES: u64 = 2 * 1024 * 1024;

/// Reads `path` off-thread for the Explorer's live preview, the same
/// physical work as [`read_file_cmd`] but with the preview's own tighter
/// size gate ([`MAX_PREVIEW_BYTES`], enforced by `rune_vfs::get` before
/// reading) and a UTF-8 validity check so a binary file never reaches
/// `Buffer::from_bytes`. Every rejection reports through the SAME `Result`
/// channel `read_file_cmd` uses (`Msg::FileOpened`'s `result`) rather than a
/// distinct error shape — `explorer_preview::maybe_consume_reply` is the
/// only reader, and it treats every `Err` here identically: silently keep
/// showing whatever was previewed before, never the ordinary open-failure
/// banner `workspace::handle_file_opened` would otherwise raise. `anchor`
/// is always `None`: a preview never lands a navigation anchor.
pub fn read_preview_cmd(vfs: Arc<dyn Vfs + Send + Sync>, path: PathBuf) -> Cmd {
    Cmd::read_file(move || {
        let result = (|| -> Result<Vec<u8>, String> {
            let bytes = match rune_vfs::get(vfs.as_ref(), &path, Some(MAX_PREVIEW_BYTES)) {
                Ok(sighting) => sighting.bytes,
                Err(rune_vfs::GetRefusal::TooLarge { .. }) => {
                    return Err("too large to preview".to_string());
                }
                Err(e) => return Err(e.to_string()),
            };
            if std::str::from_utf8(&bytes).is_err() {
                return Err("not valid UTF-8".to_string());
            }
            Ok(bytes)
        })();
        Some(Msg::FileOpened {
            path,
            result,
            anchor: None,
        })
    })
}

mod bootstrap;

// The tree-sitter highlight `Cmd` constructor and the region pass behind it
// moved to `runtime::highlight_cmd` (500-line budget) — re-exported below so
// every existing `runtime::` call site keeps working unchanged.
mod highlight_cmd;
pub(crate) use highlight_cmd::{FIRST_PAINT_BUDGET, PassBudget, highlight_cmd, run_regions};
pub use highlight_cmd::{PARSE_BUDGET, PASS_BUDGET};

// The comrak reveal-emit reuse path a ```markdown fence highlights through
// — its own file since it pulls in `rune_md::parse`/`emit`, a dependency
// `highlight_cmd` itself has no other reason to carry. Reached only from
// `highlight_cmd::run_regions`, never re-exported.
mod md_fence;

// The snapshot-autosave debounce's one rearmable timer thread —
// split out for the same reason `highlight_cmd` was: a distinct
// concern with its own `#[cfg(test)]` module, kept out of this file's own
// 500-line budget.
mod snapshot_timer;
pub use snapshot_timer::SnapshotTimer;

// The fuzzy file finder's recents-load `Cmd` constructor — split out for
// the same reason `highlight_cmd`/`snapshot_timer` were (500-line budget).
mod filesearch_recents_cmd;
pub use filesearch_recents_cmd::load_filesearch_recents_cmd;
// The fuzzy file finder's workspace-walk `Cmd` — split out for the same
// 500-line-budget reason; `filesearch::open` is its only caller, reached
// through `runtime::` like every other `Cmd` constructor in this file.
mod filesearch_cmd;
pub(crate) use filesearch_cmd::filesearch_scan_cmd;

fn translate_event(event: termina::Event) -> Option<Msg> {
    match event {
        termina::Event::Key(key) => keymap::from_termina(key).map(Msg::Key),
        termina::Event::Paste(text) => Some(Msg::Paste(text)),
        termina::Event::WindowResized(size) => Some(Msg::Resize(size.cols, size.rows)),
        termina::Event::Mouse(mouse) => crate::pointer::from_termina(mouse).map(Msg::Mouse),
        _ => None,
    }
}

#[cfg(test)]
mod translate_event_tests {
    use termina::escape::csi::{Csi, Cursor};

    use super::translate_event;

    #[test]
    fn a_terminal_reply_nobody_asked_for_produces_no_message() {
        let event = termina::Event::Csi(Csi::Cursor(Cursor::RequestActivePositionReport));
        assert!(translate_event(event).is_none());
    }

    #[test]
    fn a_focus_change_stays_silent() {
        assert!(translate_event(termina::Event::FocusIn).is_none());
        assert!(translate_event(termina::Event::FocusOut).is_none());
    }
}
