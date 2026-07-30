//! The Elm-style runtime: `Msg`, `Cmd`, `Effects`, and the main loop (plan
//! Context, "Msg/Cmd runtime"). This module's `run` (main: recv -> drain
//! `try_iter` -> `update` per message -> drain `Effects.raw` to the terminal
//! -> spawn `Effects.cmds` -> draw once), the input reader spawned by `run`,
//! one `std::thread` per `Cmd`, and `App::snapshot_timer`'s own single
//! long-lived rearmable timer thread (plan WP16.S5) — the one background
//! thread NOT spawned fresh per `Cmd`, since re-arming it is a plain state
//! update rather than new off-thread work.
//!
//! `update` mutates `App` synchronously (CONSTITUTION §5.4: "mutate
//! synchronous state directly in `update`; a Cmd is exclusively for I/O that
//! leaves the thread"). `Effects.raw` is the ONLY path by which escape bytes
//! (OSC 52 clipboard writes) reach the terminal — a `Cmd` never touches it
//! (plan Gotchas: "Cmds must never touch the terminal"; termina's `Terminal`
//! is `io::Write` on `&mut self`, single-owner, undocumented for cross-
//! thread use).

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use rune_vfs::{DirEntry, Vfs};

use crate::app::{self, App};
use crate::document::DocumentId;
use crate::keymap::{self, KeyInput};
use crate::pointer::MouseInput;
use crate::term::Guard;

/// Re-exported so a consumer of `Msg::Highlighted` can name the payload
/// type without taking its own dependency on the producer crate.
pub use rune_ts::HighlightResult;

/// One runtime event. `Key`/`Paste`/`Resize`/`Mouse` originate from the
/// input-reader thread; `ClipboardRead`/`SaveDone`/`ConfirmTimeout`/
/// `SaveConfirmTimeout` originate from a spawned `Cmd`'s return value;
/// `SnapshotDue` originates from `App::snapshot_timer`'s one long-lived
/// rearmable timer thread (plan WP16.S5), not a per-message spawned `Cmd`;
/// `Db` originates from the `rune-db` writer thread via `db::DbBridge`
/// (plan WP5.S1); `Error`/`Quit` can be synthesized by `update` itself.
/// `SaveDone`/`SnapshotDue` carry a `DocumentId` (plan WP1.S3) so multi-
/// document acks route back to the document that triggered them;
/// `ConfirmTimeout`/`SaveConfirmTimeout` stay doc-agnostic — `pending_quit`
/// is app-wide and `pending_save_confirm`'s doc tag lives in the `Option`
/// tuple itself, not in the `Msg`.
#[derive(Debug)]
pub enum Msg {
    Key(KeyInput),
    Paste(String),
    Resize(u16, u16),
    /// A mouse event, translated from `termina::Event::Mouse` (plan
    /// WP7.S4) — `commands::mouse::handle` is its sole handler.
    Mouse(MouseInput),
    ClipboardRead(String),
    SaveDone {
        id: DocumentId,
        version: u64,
        result: Result<(), String>,
    },
    ConfirmTimeout {
        generation: u32,
    },
    /// The 2s degraded-save confirm-gate timer (plan WP5.S2/S6, mirroring
    /// `ConfirmTimeout`'s quit-confirm shape) — a stale generation is
    /// ignored exactly like `ConfirmTimeout`.
    SaveConfirmTimeout {
        generation: u32,
    },
    /// The 2s snapshot-autosave debounce timer (plan WP5.S6, port of
    /// `workspace_timers.go`) — a stale generation (a later journal
    /// mutation already rescheduled) is ignored.
    SnapshotDue {
        id: DocumentId,
        generation: u32,
    },
    /// A completion posted by `rune-db`'s writer thread, routed through
    /// `db::DbBridge` (plan WP5.S1).
    Db(rune_db::DbEvent),
    /// WP7: the caller-side `vfs` `Cmd` a `MaterializePrepare` ack spawned
    /// (`save::materialize_vfs_cmd`) has finished the ENTIRE disk dance —
    /// resolve/read/hash-compare/publish/read-displaced — through this
    /// app's own `Vfs` handle, never the writer thread's. Routed to
    /// `materialize_ack::handle_materialize_vfs_done`.
    MaterializeVfsDone {
        id: DocumentId,
        outcome: crate::materialize_ack::MaterializeVfsOutcome,
    },
    /// `vfs.read_dir(root)` completed (plan WP4.S4) — the Explorer's own
    /// boundary Msg, delivered by [`load_dir_cmd`]. `Nav` (navigated into
    /// `root`) resets the Explorer's cursor to the top; `Refresh` (a future
    /// watcher-triggered reload, out of scope for WP4 itself — no
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
        generation: u32,
        result: Result<rune_db::RenameOutcome, String>,
    },
    /// A `ReadFile` `Cmd` completed (plan WP5.S6, [rune-tui A 7]) —
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
    /// A background highlight call completed (plan WP5.S2; payload split in
    /// two by the syntax-highlighting-latency plan's WP3, D6). `result: None`
    /// means NO RESULT — the parse budget elapsed, the language was
    /// unrecognised, or the parse failed — and is distinguishable from
    /// `Some(..)` carrying an empty span list, a real empty result: `None`
    /// must leave the document's previously stored `tree`/`spans` untouched,
    /// or a document whose parse is slower than the budget would lose its
    /// colours on every keystroke and never regain them. `version` is the
    /// buffer version the highlight ran against; a reply whose `version` no
    /// longer matches the live buffer is dropped the same way.
    Highlighted {
        doc: DocumentId,
        version: u64,
        result: Option<HighlightPayload>,
    },
    Error(String),
    Quit,
}

/// `Msg::Highlighted`'s payload (D6): a whole code document's background
/// parse retains the `Tree` for per-frame viewport queries; a markdown
/// document's fences (and the session fuzzer's injected hostile spans, which
/// have no way to synthesize a `ParsedTree`) keep flowing through the
/// existing span list. `dispatch::handle_highlighted` stores whichever arm
/// arrived into the matching `HighlightState` field.
#[derive(Debug)]
pub enum HighlightPayload {
    Tree(rune_ts::ParsedTree),
    Spans(HighlightResult),
}

/// Why a `Msg::DirLoaded` was requested (plan WP4.S4) — `explorer::
/// handle_dir_loaded` reacts differently: `Nav` (the user navigated to a
/// new root — Enter on a directory, Backspace to the parent, or the initial
/// `^x` load) resets the cursor to the top; `Refresh` (reserved for a later
/// fsnotify-driven reload — out of scope here per the plan's "Out of
/// scope") preserves the currently selected entry by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirCause {
    Nav,
    Refresh,
}

/// What kind of off-thread work a `Cmd` performs. Exists so a consumer that
/// must NOT execute certain effects (a headless driver: `QuitTimeout` sleeps
/// 2 real seconds, `ClipboardRead` forks `/usr/bin/pbpaste`) can decide by
/// inspection instead of inferring it from `App` field diffs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdKind {
    /// `vfs.save_atomic` — the §1.4.1 durable publish.
    Save,
    /// The 2s quit-confirm timer. Sleeps; never run it inline.
    QuitTimeout,
    /// `/usr/bin/pbpaste`. Spawns a subprocess and reads the live OS
    /// clipboard; never run it inline.
    ClipboardRead,
    /// The 2s degraded-save confirm-gate timer (plan WP5.S2/S6). Sleeps;
    /// never run it inline.
    SaveConfirmTimeout,
    /// `vfs.rename_excl` (a rename) or `write_durable` + `rename_excl` (a
    /// draft create) for the no-store route — the §1.4.1 no-clobber atomic
    /// publish. Off-thread per §5.4.
    Rename,
    /// `vfs.read_dir` for the Explorer pane (plan WP4.S4). Not a sleeping/
    /// forking `Cmd` like the three above, but still off-thread per §5.4 so
    /// a slow or degraded filesystem (an NFS mount, a huge directory) never
    /// blocks the main loop.
    ReadDir,
    /// `vfs.read` for a single FILE (plan WP5.S6, [rune-tui A 7]) —
    /// `workspace::open_path_async`'s own off-thread read, the `ReadDir`
    /// sibling for opening (rather than listing) a path: a slow or
    /// degraded filesystem must never block the main loop just because the
    /// target happens to be a file instead of a directory.
    ReadFile,
    /// `/usr/bin/open` on an external link's URL (plan WP5.S6). Spawns a
    /// subprocess; never run it inline. The session fuzzer's driver keeps
    /// only `CmdKind::Save` and drops every other `Cmd`, so this can never
    /// be spawned from a fuzz run.
    OpenExternal,
    /// A tree-sitter parse (`rune_ts::parse`, whole code documents,
    /// [`PARSE_BUDGET`]) or highlight query run (fences, [`HIGHLIGHT_BUDGET`])
    /// — plan WP5.S2, budgets split by the syntax-highlighting-latency
    /// plan's D5. Off-thread per §5.4: a large document's parse must never
    /// block the main loop, and grammar crashes (`ts_assert`) are
    /// architecturally avoided rather than caught (CONSTITUTION §1.3) —
    /// every parse is a full parse, never an incremental reparse fed a
    /// prior edit's location. The ONE sanctioned exception is a single
    /// bounded synchronous attempt at the startup document, made from
    /// `runtime::run`'s bootstrap strictly before the first draw — see
    /// `highlight::first_paint_highlight` — where nothing is on screen yet
    /// to block.
    Highlight,
}

/// Off-thread work `update` asks the runtime to perform, spawned one
/// `std::thread` each. Returns the `Msg` to feed back once the work
/// completes, or `None` to produce nothing.
pub struct Cmd {
    kind: CmdKind,
    run: Box<dyn FnOnce() -> Option<Msg> + Send + 'static>,
}

impl Cmd {
    pub fn new(kind: CmdKind, run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Cmd {
            kind,
            run: Box::new(run),
        }
    }

    pub fn kind(&self) -> CmdKind {
        self.kind
    }

    pub fn run(self) -> Option<Msg> {
        (self.run)()
    }
}

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
}

/// Runs the editor until the user quits or the input stream ends. Owns the
/// terminal for the lifetime of this call: `term::Guard` wraps a
/// `termina::Terminal`, single-owner and main-thread-only by the crate's own
/// design (see module docs).
pub fn run(app: &mut App) -> io::Result<()> {
    let mut guard = Guard::new()?;

    // Plan WP4.S5: probe BEFORE `spawn_input_reader` starts consuming
    // events on its own thread — the probe's own poll/read round trip
    // over the DA1 query would otherwise race that thread for the same
    // input stream (the "typed Csi response" it waits for could be
    // delivered to either reader). One-shot, at startup, never per frame.
    app.theme = crate::theme::Theme::catppuccin_mocha(!guard.probe_truecolor());

    let (tx, rx) = mpsc::channel::<Msg>();
    spawn_input_reader(guard.event_reader(), tx.clone());

    // Hand the runtime's own `Sender<Msg>` to the DB bridge (plan WP5.S1's
    // "App-held setter" — `Store::open`, at bootstrap in `rune-cli::main`,
    // ran before this `Sender<Msg>` ever existed, see `db::DbBridge`'s doc
    // comment) so every `DbEvent` from here on is delivered as `Msg::Db`
    // through the ordinary Elm loop below, exactly like the initial
    // `Msg::Resize` seed right after it.
    if let Some(db) = &app.db {
        db.bridge.attach(tx.clone());
    }

    // Same "App-held setter" pattern as `db.bridge.attach` right above:
    // `App::new` constructs `snapshot_timer` with no background thread at
    // all (plan WP16.S5), so every test/fuzz `App` that never reaches this
    // `run` loop never spawns one either. This call starts its one thread.
    app.snapshot_timer.attach(tx.clone());
    // Tracks the join handle of every no-store fallback save `Cmd`
    // (`CmdKind::Save`) currently running, so quitting can join them instead
    // of detaching them mid-write (review fix, [rune-tui A 5]): a
    // store-backed materialize survives process exit via `Store::shutdown`'s
    // own drain, but the no-store fallback is a plain detached thread doing
    // its own `vfs.save_atomic` — `thread::spawn`'s `JoinHandle`, dropped,
    // detaches and keeps running past `main` returning, but the atomic
    // publish it's mid-write on has no other guarantee of finishing before
    // the process actually exits. Pruned opportunistically so this never
    // grows unbounded across a long session of saves.
    let mut save_handles: Vec<thread::JoinHandle<()>> = Vec::new();

    // Seed the initial size through the ordinary `update` path (not a
    // one-off field write) so `Msg::Resize`'s effect on the viewport has
    // exactly one implementation, exercised the same way on every resize.
    let (width, height) = guard.size()?;
    apply(
        app,
        Msg::Resize(width, height),
        &mut guard,
        &tx,
        &mut save_handles,
    )?;

    // D4 (syntax-highlighting-latency plan): one bounded synchronous parse
    // attempt at the startup document, strictly before the first draw below
    // — nothing is on screen yet, so even a full-budget miss blocks nothing
    // visible. A hit means frame 1 renders already highlighted; a miss (or
    // a non-code startup document) falls through to the ordinary background
    // kick right after, unchanged.
    crate::highlight::first_paint_highlight(app);

    // Plan WP5.S3, "App::new's bootstrap path": `App::new` itself has no
    // `&mut Effects` to dispatch a highlight `Cmd` with (it runs before this
    // runtime, and before any `Msg` has ever reached `app::update`'s
    // before/after gate), so the document it opened with never gets its
    // first highlight kicked from there. This is the earliest point that
    // both an `App` and an `Effects` sink exist together, so it is the one
    // explicit kick this bootstrap path needs; every later document (an
    // edit, a tab switch, `workspace::open_path`) is already covered by
    // `app::update`'s own before/after gate. When `first_paint_highlight`
    // just succeeded for this document, `schedule_highlight`'s own
    // already-current guard (`highlight.version == version`) makes this a
    // no-op — see that function's doc comment.
    // Same bootstrap-window reasoning as the highlight kick above, for the
    // same reason: a launch with no file to edit shows the left column
    // before any key is pressed, but the constructor had no `Effects` to
    // request the listing with, so without this the pane would render as an
    // empty box until the user pressed the focus chord. A no-op whenever the
    // column starts hidden or the Explorer already has entries.
    {
        let mut effects = Effects::default();
        crate::highlight::schedule_highlight(app, app.active, &mut effects);
        crate::explorer::ensure_loaded(app, &mut effects);
        for cmd in effects.cmds.drain(..) {
            spawn_cmd(cmd, tx.clone(), &mut save_handles);
        }
    }

    app.sync_view();
    guard.draw(|frame| crate::render::draw(app, frame))?;

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
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
    for raw in effects.raw.drain(..) {
        guard.write_raw(&raw)?;
    }
    for cmd in effects.cmds.drain(..) {
        spawn_cmd(cmd, tx.clone(), save_handles);
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

/// Reads `root`'s children off-thread via `vfs.read_dir` (plan WP4.S4) and
/// replies with `Msg::DirLoaded`, or `Msg::Error` on a read failure — the
/// Explorer's own boundary Msg, called from `explorer::handle_key` (Open on
/// a directory, Backspace to the parent) and from `pane::handle_global_
/// command`'s `FocusExplorer` arm (the very first load). §1.4.9: the
/// filesystem is reached only through the injected `Vfs`; §5.4: this I/O
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
    Cmd::new(CmdKind::ReadDir, move || match vfs.read_dir(&root) {
        Ok(entries) => Some(Msg::DirLoaded {
            root,
            entries,
            cause,
            generation,
        }),
        Err(e) => Some(Msg::Error(format!(
            "could not list {}: {e}",
            root.display()
        ))),
    })
}

/// Reads `path` off-thread via `vfs.read` (plan WP5.S6, [rune-tui A 7]) —
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
    Cmd::new(CmdKind::ReadFile, move || {
        let result = vfs.read(&path).map_err(|e| e.to_string());
        Some(Msg::FileOpened {
            path,
            result,
            anchor,
        })
    })
}

// The tree-sitter highlight `Cmd` constructors (`highlight_cmd`, `fence_
// highlight_cmd`, plus `HIGHLIGHT_BUDGET`/`PARSE_BUDGET`) moved to
// `runtime::highlight_cmd` (§1.6 budget) — re-exported below so every
// existing `runtime::` call site keeps working unchanged.
mod highlight_cmd;
pub(crate) use highlight_cmd::FIRST_PAINT_BUDGET;
pub use highlight_cmd::{HIGHLIGHT_BUDGET, PARSE_BUDGET, fence_highlight_cmd, highlight_cmd};

// The snapshot-autosave debounce's one rearmable timer thread (plan
// WP16.S5) — split out for the same reason `highlight_cmd` was: a distinct
// concern with its own `#[cfg(test)]` module, kept out of this file's own
// §1.6 budget.
mod snapshot_timer;
pub use snapshot_timer::SnapshotTimer;

fn translate_event(event: termina::Event) -> Option<Msg> {
    match event {
        termina::Event::Key(key) => keymap::from_termina(key).map(Msg::Key),
        termina::Event::Paste(text) => Some(Msg::Paste(text)),
        termina::Event::WindowResized(size) => Some(Msg::Resize(size.cols, size.rows)),
        termina::Event::Mouse(mouse) => crate::pointer::from_termina(mouse).map(Msg::Mouse),
        _ => None,
    }
}
