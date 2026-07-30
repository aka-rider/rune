//! WP10 performance guard: asserts the full display pipeline on a 5,000-line
//! mixed markdown document completes in under 100 ms.
//!
//! This is a wall-clock bound that must ONLY run via the explicit release
//! invocation in Make (rust-perf-guard). It is inherently flaky inside
//! ordinary parallel debug `cargo test` and is marked #[ignore] for that
//! reason.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Instant;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;

include!("shared_doc.rs.inc");

#[ignore = "This is a wall-clock bound that must ONLY run via the explicit \
            release invocation in Make (rust-perf-guard). It is inherently \
            flaky inside ordinary parallel debug `cargo test` and is marked \
            #[ignore] for that reason."]
#[test]
fn full_pipeline_5k_under_100ms() {
    let doc = build_5k_doc();

    let start = Instant::now();

    let buf = Buffer::new(&doc);
    let cursors = CursorSet::new(0);
    let mut machine = DocMachine::new();
    machine.sync_content(&buf);
    let _refs = rune_md::catalogue::catalogue(buf.content(), machine.blocks());
    machine.sync_cursors(&buf, &cursors);
    let _snap = machine.snapshot(&buf);

    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "full pipeline on 5k-line doc took {:.2} ms (budget: 100 ms)",
        elapsed.as_secs_f64() * 1_000.0
    );
}
