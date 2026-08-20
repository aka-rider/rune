#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;

use rune_tui::app;
use rune_tui::messages::Severity;
use rune_tui::runtime::{Effects, Msg, drain_batch};
use rune_tui::testgrid;

use tui_edit_common::app_for;

const PRODUCERS: usize = 4;
const PER_PRODUCER: usize = 2_000;

fn tag(producer: usize, index: usize) -> String {
    format!("{producer}:{index}")
}

fn parse_tag(text: &str) -> (usize, usize) {
    let (p, i) = text
        .split_once(':')
        .expect("every flood message carries a producer:index tag");
    (p.parse().expect("producer id"), i.parse().expect("index"))
}

#[test]
fn a_flood_from_every_producer_drains_in_full_and_in_per_producer_order() {
    let (tx, rx) = mpsc::channel::<Msg>();

    let producers: Vec<_> = (0..PRODUCERS)
        .map(|p| {
            let tx = tx.clone();
            thread::spawn(move || {
                for i in 0..PER_PRODUCER {
                    tx.send(Msg::Posted {
                        severity: Severity::Info,
                        text: tag(p, i),
                    })
                    .expect("the receiver outlives every producer in this test");
                }
            })
        })
        .collect();
    for handle in producers {
        handle
            .join()
            .expect("a flood producer thread must not panic");
    }
    drop(tx);

    let mut app = app_for("hello\n", 0);
    let mut last_index: HashMap<usize, usize> = HashMap::new();
    let mut total_received = 0usize;
    let mut batch_count = 0usize;
    let mut max_batch_len = 0usize;

    while let Some(batch) = drain_batch(&rx) {
        batch_count += 1;
        max_batch_len = max_batch_len.max(batch.len());

        for msg in batch {
            if let Msg::Posted { text, .. } = &msg {
                let (producer, index) = parse_tag(text);
                match last_index.get(&producer) {
                    None => assert_eq!(
                        index, 0,
                        "producer {producer}'s first delivered message must be its first sent one"
                    ),
                    Some(&prev) => assert_eq!(
                        index,
                        prev + 1,
                        "producer {producer}'s messages must arrive in the order it sent them"
                    ),
                }
                last_index.insert(producer, index);
                total_received += 1;
            }
            let mut effects = Effects::default();
            app::update(&mut app, msg, &mut effects);
        }
        app.sync_view();
    }

    assert_eq!(
        total_received,
        PRODUCERS * PER_PRODUCER,
        "every message the flood sent must be drained exactly once, none dropped"
    );
    for p in 0..PRODUCERS {
        assert_eq!(
            last_index.get(&p),
            Some(&(PER_PRODUCER - 1)),
            "producer {p} must have every one of its messages delivered, up to its last"
        );
    }
    assert!(
        batch_count >= 1,
        "the drain loop must make at least one pass over a non-empty channel"
    );
    assert!(
        max_batch_len > 1,
        "flooding all producers before draining must show the loop batching \
         more than one message per turn — that is the deep-backlog behavior \
         this test pins"
    );

    let grid = testgrid::draw(&app, 80, 24);
    let rendered = (0..24)
        .map(|y| {
            (0..80)
                .filter_map(|x| grid.cell((x, y)).map(ratatui::buffer::Cell::symbol))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("hello"),
        "a draw must still succeed and show the document after a deep backlog \
         is drained:\n{rendered}"
    );
}
