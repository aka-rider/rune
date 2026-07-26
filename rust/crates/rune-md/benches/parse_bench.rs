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

/// Build a deterministic 5,000-line markdown document.
fn build_5k_doc() -> String {
    let mut doc = String::with_capacity(5_000 * 40);

    // Pattern: 30 lines per cycle, repeated 166 times (4,980 lines) + 20
    // extra lines = 5,000 total.
    let pattern: [&str; 31] = [
        "# Heading One",
        "## Heading Two",
        "### Heading Three",
        "#### Heading Four",
        "##### Heading Five",
        "###### Heading Six",
        "A paragraph with **bold**, *italic*, [a link](https://example.com), [[wikilink]], and `inline code`.",
        "```rust",
        "fn main() {",
        "    println!(\"hello\");",
        "}",
        "```",
        "> This is a blockquote line.",
        "- [ ] unchecked task",
        "\u{1f525} \u{65e5} \u{1f389} \u{4e2d}\u{6587} \u{d55c}\u{ad6d}\u{c5b4}",
        "",
        "1. ordered item alpha",
        "- unordered item beta",
        "---",
        "This is a continuation paragraph with **bold** and *italic* text.",
        "Another paragraph line with a [link](https://rust-lang.org) and `code`.",
        "> Blockquote continuation on a second line.",
        "Some ~~strikethrough~~ and **bold** inline formatting here.",
        "[Another link](https://doc.rust-lang.org) on its own line.",
        "[[target|label]] wikilink on its own line.",
        "```",
        "simple code block",
        "```",
        "- [x] completed task",
        "\u{1f680} \u{1f30d} \u{1f3af} CJK mixed content line",
        "",
    ];

    let pattern_len = pattern.len(); // 31
    let full_cycles = 5_000 / pattern_len; // 166
    let remainder = 5_000 % pattern_len; // 20

    for _ in 0..full_cycles {
        for line in &pattern {
            doc.push_str(line);
            doc.push('\n');
        }
    }
    for line in &pattern[..remainder] {
        doc.push_str(line);
        doc.push('\n');
    }

    doc
}

fn full_pipeline_benchmark(c: &mut Criterion) {
    let doc = build_5k_doc();

    c.bench_function("parse_bench", |b| {
        b.iter(|| {
            let buf = Buffer::new(black_box(doc.as_str()));
            let cursors = CursorSet::new(0);
            let mut machine = DocMachine::new();
            machine.sync_content(&buf);
            machine.sync_cursors(&buf, &cursors);
            let _snap = machine.snapshot(&buf);
        });
    });
}

criterion_group!(benches, full_pipeline_benchmark);
criterion_main!(benches);
