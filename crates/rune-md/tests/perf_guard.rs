//! WP10 performance guard: asserts the full display pipeline on a 5,000-line
//! mixed markdown document completes in under 100 ms.
//!
//! This is a wall-clock bound that must ONLY run via the explicit release
//! invocation in Make (rust-perf-guard). It is inherently flaky inside
//! ordinary parallel debug `cargo test` and is marked #[ignore] for that
//! reason.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::{Duration, Instant};

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

/// The smaller of the two document sizes [`full_pipeline_cost_scales_
/// linearly_not_quadratically_with_document_size`] compares — [`LARGE_LINES`]
/// is exactly 5x this, so a perfectly linear pipeline gives a cost ratio of
/// 5, and a quadratic one (issue #11's own shape: `line_of`'s per-call
/// rescan from byte 0 of the whole document) gives roughly 25 (5 squared).
const SMALL_LINES: usize = 25_000;
const LARGE_LINES: usize = 125_000;

/// How many consecutive full-pipeline runs each size is averaged over — the
/// same AVERAGING discipline `crates/rune-tui/tests/perf_guard.rs` uses
/// (never a single cold sample), scaled down from that file's 100-200 from
/// a single-keystroke-sized cost to this gate's whole-document one:
/// `LARGE_LINES` needs to be large enough that a quadratic regression's cost
/// is unmistakable against noise (issue #11 itself was invisible at a few
/// thousand lines and severe by a few hundred thousand), and at that size
/// even a linear pipeline's own per-run cost is measured in the tens to low
/// hundreds of milliseconds — 100-200 reps of that would make this gate
/// itself minutes long. 30 still averages away scheduling noise while
/// keeping the GREEN case (what every ordinary `make perf-guard` run pays)
/// in the tens of seconds.
const SCALING_ITERATIONS: usize = 30;

/// The growth-ratio bound: `SIZE_RATIO` (5.0, exact) times this much slack.
/// Comfortably above what a linear pipeline measures (~5, some slack for
/// per-call fixed overhead skewing it further down at the smaller size) and
/// comfortably below what a quadratic regression measures (~25) — a shape
/// change, not a wall-clock number, is what this bound exists to catch.
const RATIO_SLACK: f64 = 3.0;

/// One warm-up run (excluded from the measurement, matching `rune-tui`'s
/// own perf guard discipline) plus `iterations` timed runs of the full
/// `sync_content` -> `sync_cursors` -> `snapshot` pipeline, averaged.
fn average_pipeline_cost(doc: &str, iterations: usize) -> Duration {
    let run_once = || {
        let buf = Buffer::new(doc);
        let cursors = CursorSet::new(0);
        let mut machine = DocMachine::new();
        machine.sync_content(&buf);
        machine.sync_cursors(&buf, &cursors);
        std::hint::black_box(machine.snapshot(&buf));
    };
    run_once();
    let start = Instant::now();
    for _ in 0..iterations {
        run_once();
    }
    start.elapsed() / u32::try_from(iterations).unwrap_or(1)
}

#[ignore = "This is a wall-clock bound that must ONLY run via the explicit \
            release invocation in Make (rust-perf-guard). It is inherently \
            flaky inside ordinary parallel debug `cargo test` and is marked \
            #[ignore] for that reason."]
#[test]
fn full_pipeline_cost_scales_linearly_not_quadratically_with_document_size() {
    let small = build_doc(SMALL_LINES);
    let large = build_doc(LARGE_LINES);

    let avg_small = average_pipeline_cost(&small, SCALING_ITERATIONS);
    let avg_large = average_pipeline_cost(&large, SCALING_ITERATIONS);

    let size_ratio = LARGE_LINES as f64 / SMALL_LINES as f64;
    let cost_ratio = avg_large.as_secs_f64() / avg_small.as_secs_f64().max(f64::EPSILON);
    let bound = size_ratio * RATIO_SLACK;

    assert!(
        cost_ratio < bound,
        "document size grew {size_ratio:.1}x ({SMALL_LINES} -> {LARGE_LINES} \
         lines) but the full pipeline's average cost grew {cost_ratio:.1}x \
         ({avg_small:?} -> {avg_large:?}, over {SCALING_ITERATIONS} \
         iterations each) — bound was {bound:.1}x; a ratio anywhere near \
         {:.0}x (5 squared) is the quadratic shape issue #11 fixed",
        size_ratio * size_ratio,
    );
}
