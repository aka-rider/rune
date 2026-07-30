//! Criterion benchmark: full display pipeline on a deterministic 5,000-line
//! mixed markdown document.
//!
//! Per iteration the benchmark runs the complete pipeline:
//!   1. Create a `Buffer` with the markdown content.
//!   2. Create an empty `CursorSet`.
//!   3. Create a `DocMachine`, call `sync_content`, `sync_cursors`, then
//!      `snapshot`.
//!
//! The document is **fixed** -- no randomness, no `Date::now`, no seeded
//! content. Every iteration processes the identical byte sequence so the
//! criterion median is a stable signal across runs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;

include!("../tests/shared_doc.rs.inc");

fn full_pipeline_benchmark(c: &mut Criterion) {
    let doc = build_5k_doc();

    c.bench_function("parse_bench", |b| {
        b.iter(|| {
            let buf = Buffer::new(black_box(doc.as_str()));
            let cursors = CursorSet::new(0);
            let mut machine = DocMachine::new();
            machine.sync_content(&buf);
            machine.sync_cursors(&buf, &cursors);
            black_box(machine.snapshot(&buf));
        });
    });
}

criterion_group!(benches, full_pipeline_benchmark);
criterion_main!(benches);
