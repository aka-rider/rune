use std::collections::VecDeque;

// Kitty graphics protocol: chunks of one image transmit must arrive in
// order, and no other graphics escape may appear between them.
pub const DRAIN_BUDGET_BYTES: usize = 256 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Pumped {
    Drained,
    StillOwing,
}

struct Queued {
    chunks: VecDeque<Vec<u8>>,
    is_image: bool,
    started: bool,
}

#[derive(Default)]
pub struct TransmitQueue {
    pending: VecDeque<Queued>,
}

impl TransmitQueue {
    pub fn push_image(&mut self, chunks: Vec<Vec<u8>>) {
        self.pending.push_back(Queued {
            chunks: chunks.into(),
            is_image: true,
            started: false,
        });
    }

    pub fn push_raw(&mut self, bytes: Vec<u8>) {
        self.pending.push_back(Queued {
            chunks: VecDeque::from(vec![bytes]),
            is_image: false,
            started: false,
        });
    }

    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn pump<E>(
        &mut self,
        budget: usize,
        mut write: impl FnMut(&[u8]) -> Result<(), E>,
        wake: impl FnOnce(),
    ) -> Result<Pumped, E> {
        let mut written = 0usize;
        while let Some(front) = self.pending.front_mut() {
            let Some(chunk) = front.chunks.pop_front() else {
                self.pending.pop_front();
                continue;
            };
            write(&chunk)?;
            front.started = true;
            written += chunk.len();
            if front.chunks.is_empty() {
                self.pending.pop_front();
            }
            if written >= budget {
                break;
            }
        }
        if self.is_pending() {
            wake();
            return Ok(Pumped::StillOwing);
        }
        Ok(Pumped::Drained)
    }

    pub fn flush_escapes_abandoning_images<E>(
        &mut self,
        mut write: impl FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        for queued in std::mem::take(&mut self.pending) {
            if queued.is_image {
                // Without a final empty chunk, the terminal keeps reading
                // every later escape as more of this Kitty image transfer.
                if queued.started {
                    write(rune_image::encode_transmit_terminator().as_bytes())?;
                }
                continue;
            }
            for chunk in queued.chunks {
                write(&chunk)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn chunks_of(chunks: &[&[u8]]) -> Vec<Vec<u8>> {
        chunks.iter().map(|c| c.to_vec()).collect()
    }

    fn pump_into(queue: &mut TransmitQueue, budget: usize, sink: &mut Vec<Vec<u8>>) -> Pumped {
        queue
            .pump::<std::convert::Infallible>(
                budget,
                |bytes| {
                    sink.push(bytes.to_vec());
                    Ok(())
                },
                || {},
            )
            .expect("infallible sink")
    }

    #[test]
    fn an_image_larger_than_the_budget_drains_over_several_turns_in_order() {
        let mut queue = TransmitQueue::default();
        queue.push_image(chunks_of(&[b"aaaa", b"bbbb", b"cccc"]));

        let mut sink = Vec::new();
        assert_eq!(pump_into(&mut queue, 5, &mut sink), Pumped::StillOwing);
        assert_eq!(sink, vec![b"aaaa".to_vec(), b"bbbb".to_vec()]);

        assert_eq!(pump_into(&mut queue, 5, &mut sink), Pumped::Drained);
        assert_eq!(sink.last().unwrap(), b"cccc");
        assert!(!queue.is_pending());
    }

    #[test]
    fn a_zero_budget_still_writes_one_chunk_so_the_queue_cannot_stall() {
        let mut queue = TransmitQueue::default();
        queue.push_image(chunks_of(&[b"a", b"b"]));

        let mut sink = Vec::new();
        assert_eq!(pump_into(&mut queue, 0, &mut sink), Pumped::StillOwing);
        assert_eq!(sink, vec![b"a".to_vec()]);
    }

    #[test]
    fn a_turn_that_finishes_one_image_and_starts_the_next_still_owes_the_terminal() {
        let mut queue = TransmitQueue::default();
        queue.push_image(chunks_of(&[b"aaaa"]));
        queue.push_image(chunks_of(&[b"bbbb", b"cccc"]));

        let mut sink = Vec::new();
        assert_eq!(pump_into(&mut queue, 6, &mut sink), Pumped::StillOwing);
        assert_eq!(sink, vec![b"aaaa".to_vec(), b"bbbb".to_vec()]);
        assert_eq!(pump_into(&mut queue, 6, &mut sink), Pumped::Drained);
    }

    #[test]
    fn the_last_turn_of_a_drain_wakes_nobody() {
        let mut queue = TransmitQueue::default();
        queue.push_image(chunks_of(&[b"aaaa", b"bbbb"]));

        let mut wakes = 0usize;
        let turn = |queue: &mut TransmitQueue, wakes: &mut usize| {
            queue
                .pump::<std::convert::Infallible>(1, |_| Ok(()), || *wakes += 1)
                .expect("infallible sink")
        };
        assert_eq!(turn(&mut queue, &mut wakes), Pumped::StillOwing);
        assert_eq!(turn(&mut queue, &mut wakes), Pumped::Drained);
        assert_eq!(wakes, 1);
    }

    #[test]
    fn two_queued_images_never_interleave_their_chunks() {
        let mut queue = TransmitQueue::default();
        queue.push_image(chunks_of(&[b"1111", b"2222"]));
        queue.push_image(chunks_of(&[b"3333", b"4444"]));

        let mut sink = Vec::new();
        while queue.is_pending() {
            pump_into(&mut queue, 1, &mut sink);
        }
        assert_eq!(
            sink,
            vec![
                b"1111".to_vec(),
                b"2222".to_vec(),
                b"3333".to_vec(),
                b"4444".to_vec()
            ]
        );
    }

    #[test]
    fn an_escape_produced_mid_transmit_lands_after_the_image_it_arrived_during() {
        let mut queue = TransmitQueue::default();
        queue.push_image(chunks_of(&[b"1111", b"2222", b"3333"]));

        let mut sink = Vec::new();
        pump_into(&mut queue, 1, &mut sink);
        queue.push_raw(b"osc".to_vec());
        while queue.is_pending() {
            pump_into(&mut queue, 1, &mut sink);
        }

        assert_eq!(
            sink,
            vec![
                b"1111".to_vec(),
                b"2222".to_vec(),
                b"3333".to_vec(),
                b"osc".to_vec()
            ]
        );
    }

    #[test]
    fn quitting_writes_every_queued_escape_in_order_and_abandons_image_chunks() {
        let mut queue = TransmitQueue::default();
        queue.push_image(chunks_of(&[b"1111", b"2222"]));
        queue.push_raw(b"osc-first".to_vec());
        queue.push_image(chunks_of(&[b"3333"]));
        queue.push_raw(b"osc-second".to_vec());

        let mut sink: Vec<Vec<u8>> = Vec::new();
        queue
            .flush_escapes_abandoning_images::<std::convert::Infallible>(|bytes| {
                sink.push(bytes.to_vec());
                Ok(())
            })
            .expect("infallible sink");

        assert_eq!(sink, vec![b"osc-first".to_vec(), b"osc-second".to_vec()]);
        assert!(!queue.is_pending());
    }

    #[test]
    fn quitting_terminates_a_half_written_image_before_the_escapes_behind_it() {
        let mut queue = TransmitQueue::default();
        queue.push_image(chunks_of(&[b"1111", b"2222", b"3333"]));
        queue.push_raw(b"osc-first".to_vec());
        queue.push_raw(b"osc-second".to_vec());

        let mut sink: Vec<Vec<u8>> = Vec::new();
        pump_into(&mut queue, 1, &mut sink);
        sink.clear();

        queue
            .flush_escapes_abandoning_images::<std::convert::Infallible>(|bytes| {
                sink.push(bytes.to_vec());
                Ok(())
            })
            .expect("infallible sink");

        assert_eq!(
            sink,
            vec![
                rune_image::encode_transmit_terminator().into_bytes(),
                b"osc-first".to_vec(),
                b"osc-second".to_vec()
            ]
        );
    }

    #[test]
    fn a_write_failure_leaves_the_rest_of_the_image_queued() {
        let mut queue = TransmitQueue::default();
        queue.push_image(chunks_of(&[b"aaaa", b"bbbb"]));

        let err = queue
            .pump(usize::MAX, |_| Err("pty gone"), || {})
            .unwrap_err();
        assert_eq!(err, "pty gone");
        assert!(queue.is_pending());
    }
}
