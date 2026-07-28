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

/// Build the same deterministic 5,000-line markdown document as the
/// criterion benchmark (benches/parse_bench.rs), EXTENDED (plan WP4.S6) with
/// a GFM table in every cycle of the repeating pattern: `Makefile`'s
/// `perf-guard` target pins `--exact full_pipeline_5k_under_100ms`, so a
/// *new* test asserting table performance would be silently filtered out by
/// that `--exact` match and the gate would stay green while measuring
/// nothing — the only way to actually cover table rendering under this gate
/// is to make the 5,000-line document this EXISTING test already builds
/// contain a substantial run of table rows itself. One 4-line table
/// (header, delimiter, two body rows) is folded into the pattern below,
/// which now repeats ~140 times across 5,000 lines — roughly 560 table
/// lines total, exercising `emit_table`'s Grid layout selection, cell
/// rendering, and column-width computation on every single cycle, not just
/// once.
///
/// Duplicated here so the perf guard is a self-contained integration test
/// that doesn't depend on the benchmark crate being compiled.
fn build_5k_doc() -> String {
    let mut doc = String::with_capacity(5_000 * 40);

    let pattern: [&str; 36] = [
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
        "| Name | Role | Notes |",
        "| :--- | :---: | ---: |",
        "| Alice | **Lead** | on the `api` team |",
        "| Bob | Reviewer | works on [docs](https://example.com/docs) |",
        "",
    ];

    let full_cycles = 5_000 / pattern.len();
    let remainder = 5_000 % pattern.len();

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
    machine.sync_cursors(&buf, &cursors);
    let _snap = machine.snapshot(&buf);

    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "full pipeline on 5k-line doc took {:.2} ms (budget: 100 ms)",
        elapsed.as_secs_f64() * 1_000.0
    );
}
