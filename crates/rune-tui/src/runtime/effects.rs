use std::io;
use std::sync::mpsc;
use std::thread;

use super::pool::Pool;
use super::transmit_queue::TransmitQueue;
use super::{Cmd, Msg, spawn_cmd};
use crate::term::Guard;

pub enum Outbound {
    Raw(Vec<u8>),
    Transmit(rune_image::Transmit),
}

#[derive(Default)]
pub struct Effects {
    pub cmds: Vec<Cmd>,
    pub out: Vec<Outbound>,
    pub force_redraw: bool,
}

impl Effects {
    pub fn write(&mut self, bytes: Vec<u8>) {
        self.out.push(Outbound::Raw(bytes));
    }

    pub fn transmit(&mut self, transmit: rune_image::Transmit) {
        self.out.push(Outbound::Transmit(transmit));
    }

    pub fn raw_bytes(&self) -> Vec<Vec<u8>> {
        self.out
            .iter()
            .filter_map(|out| match out {
                Outbound::Raw(bytes) => Some(bytes.clone()),
                Outbound::Transmit(_) => None,
            })
            .collect()
    }

    pub fn transmits(&self) -> Vec<&rune_image::Transmit> {
        self.out
            .iter()
            .filter_map(|out| match out {
                Outbound::Transmit(transmit) => Some(transmit),
                Outbound::Raw(_) => None,
            })
            .collect()
    }
}

#[derive(Default)]
pub struct RedrawLatch {
    requested: bool,
}

impl RedrawLatch {
    pub fn request(&mut self) {
        self.requested = true;
    }

    pub fn take(&mut self) -> bool {
        std::mem::take(&mut self.requested)
    }
}

pub(crate) struct Sink {
    pub guard: Guard,
    pub transmits: TransmitQueue,
    pub redraw: RedrawLatch,
    pub pool: Pool,
}

impl Sink {
    pub fn redraw_before_draw(&mut self) {
        if self.redraw.take() {
            self.guard.force_redraw();
        }
    }
}

pub(crate) fn discharge(
    effects: &mut Effects,
    sink: &mut Sink,
    tx: &mpsc::Sender<Msg>,
    save_handles: &mut Vec<thread::JoinHandle<()>>,
) -> io::Result<()> {
    for out in effects.out.drain(..) {
        match out {
            Outbound::Raw(bytes) if !sink.transmits.is_pending() => {
                sink.guard.write_raw(&bytes)?;
            }
            Outbound::Raw(bytes) => sink.transmits.push_raw(bytes),
            Outbound::Transmit(transmit) => sink.transmits.push_image(transmit.into_chunks()),
        }
    }
    for cmd in effects.cmds.drain(..) {
        spawn_cmd(cmd, tx.clone(), save_handles, &sink.pool);
    }
    if effects.force_redraw {
        sink.redraw.request();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::RedrawLatch;

    #[test]
    fn a_request_survives_every_turn_whose_frame_was_suppressed() {
        let mut latch = RedrawLatch::default();
        latch.request();
        latch.request();
        assert!(latch.take());
    }

    #[test]
    fn a_drawn_frame_clears_the_latch_so_the_next_one_does_not_redraw() {
        let mut latch = RedrawLatch::default();
        latch.request();
        assert!(latch.take());
        assert!(!latch.take());
    }

    #[test]
    fn a_frame_nobody_asked_to_redraw_does_not_redraw() {
        assert!(!RedrawLatch::default().take());
    }
}
