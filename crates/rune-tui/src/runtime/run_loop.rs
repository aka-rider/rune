use std::io;
use std::sync::mpsc;
use std::thread;

use crate::app::{self, App};
use crate::keymap;

use super::bootstrap;
use super::exit_settle;
use super::pool::{Pool, run_and_reply};
use super::transmit_queue;
use super::{Cmd, CmdKind, Effects, Msg, Pumped, Sink, discharge};

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

    while let Some(batch) = drain_batch(&rx) {
        match turn(app, batch, &mut sink, &tx, &mut save_handles) {
            Ok(Turn::Continue) => {}
            Ok(Turn::Quit) => break,
            Err(e) => {
                fatal = Err(e);
                break;
            }
        }
    }

    let flushed = sink
        .transmits
        .flush_escapes_abandoning_images(|bytes| sink.guard.write_raw(bytes));
    exit_settle::join_save_handles(app, &rx, &mut save_handles);

    fatal.and(flushed)
}

pub const MAX_TURN_BATCH: usize = 256;

pub fn drain_batch(rx: &mpsc::Receiver<Msg>) -> Option<Vec<Msg>> {
    let first = rx.recv().ok()?;
    let mut batch = vec![first];
    while batch.len() < MAX_TURN_BATCH {
        match rx.try_recv() {
            Ok(msg) => batch.push(msg),
            Err(_) => break,
        }
    }
    Some(batch)
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

pub(super) fn apply(
    app: &mut App,
    msg: Msg,
    sink: &mut Sink,
    tx: &mpsc::Sender<Msg>,
    save_handles: &mut Vec<thread::JoinHandle<()>>,
) -> io::Result<()> {
    let is_resize = matches!(msg, Msg::Resize(_, _));
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
    discharge(&mut effects, sink, tx, save_handles)?;
    if is_resize {
        crate::graphics::redetect(app, &mut sink.guard);
    }
    Ok(())
}

/// `Save` alone still gets its own dedicated, joinable OS thread: it is the
/// one `Cmd` kind `exit_settle::join_save_handles` waits on by name at
/// shutdown, so its thread must stay a `JoinHandle` the shutdown loop can
/// poll, never a job sitting in the bounded pool's queue behind unrelated
/// work. Every other kind — the ones that fire at keystroke rate
/// (`Highlight`, `ImageDecode`) and used to get their own unbounded thread
/// each — goes through `pool` instead, capped at a fixed worker count.
pub(super) fn spawn_cmd(
    cmd: Cmd,
    tx: mpsc::Sender<Msg>,
    save_handles: &mut Vec<thread::JoinHandle<()>>,
    pool: &Pool,
) {
    if cmd.kind() == CmdKind::Save {
        let handle = thread::spawn(move || run_and_reply(cmd, &tx));
        save_handles.retain(|h| !h.is_finished());
        save_handles.push(handle);
    } else {
        pool.submit(cmd);
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
                        return;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Msg::Posted {
                        severity: crate::messages::Severity::Error,
                        text: format!("input stream ended: {e}"),
                    });
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
