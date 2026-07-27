//! The Elm-style runtime: `Msg`, `Cmd`, `Effects`, and the three-thread main
//! loop (plan Context, "Msg/Cmd runtime"). Exactly three threads: this
//! module's `run` (main: recv -> drain `try_iter` -> `update` per message ->
//! drain `Effects.raw` to the terminal -> spawn `Effects.cmds` -> draw once),
//! the input reader spawned by `run`, and one `std::thread` per `Cmd`.
//!
//! `update` mutates `App` synchronously (CONSTITUTION §5.4: "mutate
//! synchronous state directly in `update`; a Cmd is exclusively for I/O that
//! leaves the thread"). `Effects.raw` is the ONLY path by which escape bytes
//! (OSC 52 clipboard writes) reach the terminal — a `Cmd` never touches it
//! (plan Gotchas: "Cmds must never touch the terminal"; termina's `Terminal`
//! is `io::Write` on `&mut self`, single-owner, undocumented for cross-
//! thread use).

use std::io;
use std::sync::mpsc;
use std::thread;

use crate::app::{self, App};
use crate::keymap::{self, KeyInput};
use crate::term::Guard;

/// One runtime event. `Key`/`Paste`/`Resize` originate from the input-reader
/// thread; `ClipboardRead`/`SaveDone`/`ConfirmTimeout`/`SaveConfirmTimeout`/
/// `SnapshotDue` originate from a spawned `Cmd`'s return value; `Db`
/// originates from the `rune-db` writer thread via `db::DbBridge` (plan
/// WP5.S1); `Error`/`Quit` can be synthesized by `update` itself.
pub enum Msg {
    Key(KeyInput),
    Paste(String),
    Resize(u16, u16),
    ClipboardRead(String),
    SaveDone {
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
    /// `workspace_timers.go:11`) — a stale generation (a later journal
    /// mutation already rescheduled) is ignored.
    SnapshotDue {
        generation: u32,
    },
    /// A completion posted by `rune-db`'s writer thread, routed through
    /// `db::DbBridge` (plan WP5.S1).
    Db(rune_db::DbEvent),
    Error(String),
    Quit,
}

/// Off-thread work `update` asks the runtime to perform, spawned one
/// `std::thread` each. Returns the `Msg` to feed back once the work
/// completes, or `None` to produce nothing.
pub type Cmd = Box<dyn FnOnce() -> Option<Msg> + Send + 'static>;

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

    // Seed the initial size through the ordinary `update` path (not a
    // one-off field write) so `Msg::Resize`'s effect on the viewport has
    // exactly one implementation, exercised the same way on every resize.
    let (width, height) = guard.size()?;
    apply(app, Msg::Resize(width, height), &mut guard, &tx)?;

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
            apply(app, msg, &mut guard, &tx)?;
        }

        if app.should_quit {
            break;
        }

        app.sync_view();
        guard.draw(|frame| crate::render::draw(app, frame))?;
    }

    Ok(())
}

/// Runs `update` for one message and immediately discharges its `Effects` —
/// raw bytes to the terminal, `Cmd`s to their own thread. Shared by the
/// resize-seeding call above and the main loop so there is exactly one
/// "apply a message" chokepoint.
fn apply(app: &mut App, msg: Msg, guard: &mut Guard, tx: &mpsc::Sender<Msg>) -> io::Result<()> {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
    for raw in effects.raw.drain(..) {
        guard.write_raw(&raw)?;
    }
    for cmd in effects.cmds.drain(..) {
        spawn_cmd(cmd, tx.clone());
    }
    Ok(())
}

/// A panicking `Cmd` must not vanish silently — `update` might be waiting on
/// exactly this `Cmd`'s reply with no other input in flight, which would
/// otherwise leave the main loop's `rx.recv()` blocked forever. Catching the
/// unwind here and reporting it as `Msg::Error` keeps that impossible: every
/// spawned `Cmd` thread sends SOMETHING back, success, `None`, or a caught
/// panic.
fn spawn_cmd(cmd: Cmd, tx: mpsc::Sender<Msg>) {
    thread::spawn(
        move || match std::panic::catch_unwind(std::panic::AssertUnwindSafe(cmd)) {
            Ok(Some(msg)) => {
                let _ = tx.send(msg);
            }
            Ok(None) => {}
            Err(_) => {
                let _ = tx.send(Msg::Error("a background task panicked".to_string()));
            }
        },
    );
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

fn translate_event(event: termina::Event) -> Option<Msg> {
    match event {
        termina::Event::Key(key) => keymap::from_termina(key).map(Msg::Key),
        termina::Event::Paste(text) => Some(Msg::Paste(text)),
        termina::Event::WindowResized(size) => Some(Msg::Resize(size.cols, size.rows)),
        _ => None,
    }
}
