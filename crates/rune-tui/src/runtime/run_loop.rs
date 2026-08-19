use std::io;
use std::sync::mpsc;
use std::thread;

use crate::app::{self, App};
use crate::keymap;

use super::bootstrap;
use super::exit_settle;
use super::transmit_queue;
use super::{Cmd, CmdKind, Effects, Msg, Pumped, Sink, discharge};

/// Runs the editor until the user quits or the input stream ends. Owns the
/// terminal for the lifetime of this call: `term::Guard` wraps a
/// `termina::Terminal`, single-owner and main-thread-only by the crate's own
/// design (see module docs).
pub fn run(app: &mut App) -> io::Result<()> {
    let bootstrap::Bootstrap {
        mut sink,
        tx,
        rx,
        mut save_handles,
    } = bootstrap::bootstrap(app)?;

    if sink.transmits.is_pending() {
        let _ = tx.send(Msg::PumpGraphics);
    }

    let mut fatal: io::Result<()> = Ok(());

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

        match turn(app, batch, &mut sink, &tx, &mut save_handles) {
            Ok(Turn::Continue) => {}
            Ok(Turn::Quit) => break,
            Err(e) => {
                fatal = Err(e);
                break;
            }
        }
    }

    // Every exit — quit, a dead channel, a broken terminal — lands here:
    // whatever the terminal is still owed goes out, then every fallback
    // save `Cmd` spawned above is joined before `run` returns and `main`
    // drains/shuts down the store (an in-flight one finishes its atomic
    // publish; an already-finished one joins immediately). Quit is reported
    // as complete only once this returns.
    let flushed = sink
        .transmits
        .flush_escapes_abandoning_images(|bytes| sink.guard.write_raw(bytes));
    for handle in save_handles.drain(..) {
        let _ = handle.join();
    }
    exit_settle::settle_pending_materialize(app, &rx);

    fatal.and(flushed)
}

enum Turn {
    Continue,
    Quit,
}

fn turn(
    app: &mut App,
    batch: Vec<Msg>,
    sink: &mut Sink,
    tx: &mpsc::Sender<Msg>,
    save_handles: &mut Vec<thread::JoinHandle<()>>,
) -> io::Result<Turn> {
    for msg in batch {
        apply(app, msg, sink, tx, save_handles)?;
    }

    if app.should_quit {
        return Ok(Turn::Quit);
    }

    let pumped = sink.transmits.pump(
        transmit_queue::DRAIN_BUDGET_BYTES,
        |bytes| sink.guard.write_raw(bytes),
        || {
            let _ = tx.send(Msg::PumpGraphics);
        },
    )?;
    if pumped == Pumped::StillOwing {
        return Ok(Turn::Continue);
    }

    sink.redraw_before_draw();
    app.sync_view();
    sink.guard.draw(|frame| crate::render::draw(app, frame))?;
    Ok(Turn::Continue)
}

/// Runs `update` for one message and immediately discharges its `Effects` —
/// raw bytes to the terminal, `Cmd`s to their own thread. Shared by the
/// resize-seeding call above and the main loop so there is exactly one
/// "apply a message" chokepoint.
pub(super) fn apply(
    app: &mut App,
    msg: Msg,
    sink: &mut Sink,
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
    discharge(&mut effects, sink, tx, save_handles)?;
    if is_resize {
        crate::graphics::redetect(app, &mut sink.guard);
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
pub(super) fn spawn_cmd(
    cmd: Cmd,
    tx: mpsc::Sender<Msg>,
    save_handles: &mut Vec<thread::JoinHandle<()>>,
) {
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

pub(super) fn spawn_input_reader(events: termina::EventReader, tx: mpsc::Sender<Msg>) {
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
