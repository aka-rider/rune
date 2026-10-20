//! Keystroke-latency performance guard: asserts the SYNCHRONOUS
//! per-keystroke cost — one `app::update` call plus the settle-step
//! `App::sync_view` the real runtime loop runs once per message batch,
//! never any spawned `Cmd`'s own off-thread work (a highlight parse, a
//! save) — stays bounded on a large document.
//!
//! O(file) per keystroke is the deliberate design ceiling: the buffer's
//! full content copy, line-index clone, memcmp, and journal clones per
//! edit batch buy value semantics that make the undo journal and the
//! recovery store trivially correct. A rope behind the same value-semantics
//! facade is the known successor if this guard's fixture size ever stops
//! matching real documents — grow the fixture before reaching for the rope.
//!
//! Plus the PER-FRAME render cost, which `sync_view` never reaches:
//! `render::build_rows` runs once per draw, on every draw, whether or not
//! anything changed, so work that creeps into it is paid at frame rate. The
//! two shapes that path scales with each get their own gate below — a large
//! code document (one whole-buffer region) and a markdown document with many
//! fences (one tree-sitter range query per visible region).
//!
//! This is deliberately narrower than "how long until the document is
//! fully re-highlighted": the plan's own regression was work happening
//! SYNCHRONOUSLY on the UI thread before a `Cmd` was ever dispatched (the
//! unconditional `DocMachine::snapshot` rebuild, the full-buffer clone in
//! `schedule_highlight` ahead of its own in-flight gate, the thread-per-
//! keystroke snapshot-debounce spawn) — every `effects.cmds` entry this
//! test's `update` calls produce is deliberately left un-run, exactly as
//! the real runtime leaves it to a background thread, so this test never
//! measures a tree-sitter parse or a disk write.
//!
//! Mirrors `crates/rune-md/tests/perf_guard.rs`: a wall-clock bound, run
//! only via the explicit release invocation in `make perf-guard`, `#[ignore]`
//! everywhere else because it is inherently flaky inside ordinary parallel
//! debug `cargo nextest run`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use std::time::{Duration, Instant};

use rune_tui::keymap::KeyCode;
use rune_tui::runtime::Effects;
use support::{
    RENDER_FRAMES, SHIFT, app_for, average_frame_cost, build_2mb_prose_fixture,
    build_5k_line_rust_fixture, build_fenced_markdown_fixture, press, press_with,
    settle_highlights, type_one_char_at_end,
};

/// The budget one simulated keystroke's SYNCHRONOUS cost must stay under,
/// averaged over `KEYSTROKES` consecutive keystrokes into the 5k-line
/// fixture above. Generous on purpose (this guards against a regression
/// back to thread-per-keystroke spawns and unconditional full-pipeline
/// rebuilds, not a tight budget) — `rune-md`'s own sibling guard uses a
/// 100 ms bound for a single FULL pipeline run on a doc this size; this one
/// must be far cheaper per keystroke since this guard exists specifically to make
/// most of that pipeline a memoized no-op on an ordinary keystroke.
const PER_KEYSTROKE_BUDGET: Duration = Duration::from_millis(20);
const KEYSTROKES: usize = 100;

#[ignore = "This is a wall-clock bound that must ONLY run via the explicit \
            release invocation in Make (rust-perf-guard). It is inherently \
            flaky inside ordinary parallel debug `cargo nextest run` and is marked \
            #[ignore] for that reason."]
#[test]
fn keystroke_view_cost_under_budget_on_a_5k_line_code_document() {
    let mut app = app_for(&build_5k_line_rust_fixture(), "/x/big.rs");
    // Settle once before timing — the first sync after open always
    // reparses/highlights from scratch (nothing to memoize against yet);
    // only STEADY-STATE keystrokes are what this guard measures.
    app.sync_view();

    let start = Instant::now();
    let mut effects = Effects::default();
    for (i, ch) in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$"
        .chars()
        .cycle()
        .take(KEYSTROKES)
        .enumerate()
    {
        let _ = i;
        type_one_char_at_end(&mut app, ch, &mut effects);
        // Deliberately never run `effects.cmds` — see module docs: a
        // spawned Cmd's own work happens off the UI thread in production
        // and is out of scope for this guard.
        effects.cmds.clear();
        app.sync_view();
    }
    let elapsed = start.elapsed();
    let per_keystroke = elapsed / u32::try_from(KEYSTROKES).unwrap_or(1);

    assert!(
        per_keystroke < PER_KEYSTROKE_BUDGET,
        "average per-keystroke cost on a 5k-line code document was {:.2} ms \
         over {KEYSTROKES} keystrokes (budget: {:.0} ms)",
        per_keystroke.as_secs_f64() * 1_000.0,
        PER_KEYSTROKE_BUDGET.as_secs_f64() * 1_000.0
    );
}

/// What these two budgets are and are not for.
///
/// Both were set from a measurement on the reference machine (Apple Silicon,
/// release profile): 0.135 ms per frame for the code document, 0.099 ms for
/// the 200-fence markdown one. Each budget is roughly 4x that, which is wide
/// enough not to flake under load and tight enough to catch an order-of-
/// magnitude regression — a whole-document tree-sitter query, a reparse, a
/// buffer clone, anything that turns a per-frame cost into a per-frame cost
/// that scales with the DOCUMENT rather than with the 40-row viewport.
///
/// It deliberately does not pretend to catch small creep. The per-frame
/// code-region walk this gate was written alongside measured ~9 us on the
/// code fixture and ~12 us on the markdown one — under 10% of a frame, well
/// inside any budget that a loaded machine can also satisfy. Removing a cost
/// that small is worth doing and cannot be gated; keeping the path an order
/// of magnitude away from a stall is what this gate is for.
///
/// The two differ because they scale differently: the code document is one
/// whole-buffer region (one query per frame, whatever the file's size), the
/// markdown one is 200 regions of which a handful intersect the window (one
/// query per visible region, plus a walk over all of them).
const CODE_FRAME_BUDGET: Duration = Duration::from_micros(550);
const FENCED_FRAME_BUDGET: Duration = Duration::from_micros(400);

#[ignore = "This is a wall-clock bound that must ONLY run via the explicit \
            release invocation in Make (rust-perf-guard). It is inherently \
            flaky inside ordinary parallel debug `cargo nextest run` and is marked \
            #[ignore] for that reason."]
#[test]
fn render_frame_cost_under_budget_on_a_5k_line_code_document() {
    let mut app = app_for(&build_5k_line_rust_fixture(), "/x/big.rs");
    settle_highlights(&mut app);

    let per_frame = average_frame_cost(&app);

    assert!(
        per_frame < CODE_FRAME_BUDGET,
        "average render::build_rows cost on a 5k-line code document was \
         {:.3} ms over {RENDER_FRAMES} frames (budget: {:.3} ms)",
        per_frame.as_secs_f64() * 1_000.0,
        CODE_FRAME_BUDGET.as_secs_f64() * 1_000.0
    );
}

#[ignore = "This is a wall-clock bound that must ONLY run via the explicit \
            release invocation in Make (rust-perf-guard). It is inherently \
            flaky inside ordinary parallel debug `cargo nextest run` and is marked \
            #[ignore] for that reason."]
#[test]
fn render_frame_cost_under_budget_on_a_many_fence_markdown_document() {
    let mut app = app_for(&build_fenced_markdown_fixture(), "/x/notes.md");
    settle_highlights(&mut app);
    assert!(
        app.active_doc()
            .highlight
            .regions
            .iter()
            .filter(|r| r.tree.is_some())
            .count()
            > 100,
        "the fixture must actually be highlighted, or this measures nothing"
    );

    let per_frame = average_frame_cost(&app);

    assert!(
        per_frame < FENCED_FRAME_BUDGET,
        "average render::build_rows cost on a 200-fence markdown document was \
         {:.3} ms over {RENDER_FRAMES} frames (budget: {:.3} ms)",
        per_frame.as_secs_f64() * 1_000.0,
        FENCED_FRAME_BUDGET.as_secs_f64() * 1_000.0
    );
}

const BOOTSTRAP_FRAMES: usize = 20;

/// The bound one `render::draw` call may cost while `Document::view` is
/// still `None` — generous (the fallback path this exercises,
/// `render::draw_pending`, reads at most `viewport height` lines straight
/// off `Buffer` via `str::lines().take(..)`, which is bounded by the
/// viewport regardless of document size, so the true cost is a small,
/// document-size-independent constant; this budget only needs to catch a
/// regression back to running the full display pipeline synchronously).
const BOOTSTRAP_FRAME_BUDGET: Duration = Duration::from_millis(100);

#[ignore = "This is a wall-clock bound that must ONLY run via the explicit \
            release invocation in Make (rust-perf-guard). It is inherently \
            flaky inside ordinary parallel debug `cargo nextest run` and is marked \
            #[ignore] for that reason."]
#[test]
fn bootstrap_first_draw_stays_bounded_on_a_large_document() {
    let mut app = app_for(&build_2mb_prose_fixture(), "/x/big.md");
    // Mirrors `bootstrap`'s large-document branch exactly: `relayout` sizes
    // the viewport WITHOUT running the display pipeline, so `doc.view`
    // stays at its constructed default.
    app.relayout();
    assert!(
        app.active_doc().view.is_none(),
        "fixture setup must leave the view unset, the same state a real \
         bootstrap's large-document branch draws its first frame from"
    );

    let mut elapsed_total = Duration::ZERO;
    let mut buf = rune_tui::testgrid::draw(&app, 120, 40);
    for _ in 0..BOOTSTRAP_FRAMES {
        let start = Instant::now();
        buf = rune_tui::testgrid::draw(&app, 120, 40);
        elapsed_total += start.elapsed();
    }
    let per_draw = elapsed_total / u32::try_from(BOOTSTRAP_FRAMES).unwrap_or(1);

    let rendered = (0..40)
        .map(|y| {
            (0..120)
                .filter_map(|x| buf.cell((x, y)).map(ratatui::buffer::Cell::symbol))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("quick brown fox"),
        "the pre-snapshot frame must show the document's own raw text, or \
         this measures an accidentally-cheap no-op instead of the real \
         fallback path:\n{rendered}"
    );

    assert!(
        per_draw < BOOTSTRAP_FRAME_BUDGET,
        "average render::draw cost with no view yet on a 2 MiB document was \
         {:.3} ms over {BOOTSTRAP_FRAMES} draws (budget: {:.0} ms)",
        per_draw.as_secs_f64() * 1_000.0,
        BOOTSTRAP_FRAME_BUDGET.as_secs_f64() * 1_000.0
    );
}

/// The worst case of the per-frame bracket-match scan: an UNMATCHED bracket
/// under the caret near the top of the document, so the scan runs to EOF on
/// every frame and this budget is compared against the whole 5k-line file
/// rather than the 40-row window. Same shape as the two budgets above —
/// roughly 4x a measurement (0.50 ms per frame, against 0.08 ms for the same
/// document with the caret off any bracket), wide enough not to flake and
/// tight enough to catch an order-of-magnitude regression.
const UNMATCHED_BRACKET_FRAME_BUDGET: Duration = Duration::from_micros(2_000);

#[ignore = "This is a wall-clock bound that must ONLY run via the explicit \
            release invocation in Make (rust-perf-guard). It is inherently \
            flaky inside ordinary parallel debug `cargo nextest run` and is marked \
            #[ignore] for that reason."]
#[test]
fn render_frame_cost_under_budget_with_the_caret_on_an_unmatched_bracket() {
    let fixture = format!(
        "//! A large synthetic Rust fixture for the keystroke perf guard.\n\n\
         let unmatched = (\n{}",
        build_5k_line_rust_fixture()
    );
    let mut app = app_for(&fixture, "/x/big.rs");
    settle_highlights(&mut app);

    // The caret walks to the bracket through the real update seam:
    // `settle_highlights` leaves it at end-of-buffer, so page back to the
    // top first, then two rows down onto the `let` line, to its end, and one
    // step left onto the `(`.
    let mut pages = 0usize;
    while app.active_doc().cursors().primary().position.get() > 0 {
        press(&mut app, KeyCode::PageUp);
        pages += 1;
        assert!(pages <= 500, "the caret never paged back to the top");
    }
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::End);
    press(&mut app, KeyCode::Left);

    let doc = app.active_doc();
    let at = doc.cursors().primary().position.get();
    assert_eq!(
        doc.buffer().byte(at),
        Some(b'('),
        "the caret must sit ON the unmatched bracket, or this gate measures nothing"
    );
    assert!(
        doc.has_insertion_point(),
        "the bracket pass is gated on an insertion point, or this gate measures nothing"
    );
    assert_eq!(
        rune_core::bracket::bracket_pair(doc.buffer().content(), at),
        None,
        "the fixture's bracket must stay unmatched — that is the scan-to-EOF worst case"
    );

    let per_frame = average_frame_cost(&app);

    assert!(
        per_frame < UNMATCHED_BRACKET_FRAME_BUDGET,
        "average render::build_rows cost with the caret on an unmatched bracket in a \
         5k-line code document was {:.3} ms over {RENDER_FRAMES} frames (budget: {:.3} ms)",
        per_frame.as_secs_f64() * 1_000.0,
        UNMATCHED_BRACKET_FRAME_BUDGET.as_secs_f64() * 1_000.0
    );
}

/// The worst case of the per-frame selection-match scan: a word that recurs
/// on hundreds of the document's lines, selected, so every visible line
/// carries a hit. The budget is the same one the plain code-document frame
/// is held to — the scan is bounded by the visible byte window, so selecting
/// a common word must not move the per-frame cost at all.
#[ignore = "This is a wall-clock bound that must ONLY run via the explicit \
            release invocation in Make (rust-perf-guard). It is inherently \
            flaky inside ordinary parallel debug `cargo nextest run` and is marked \
            #[ignore] for that reason."]
#[test]
fn render_frame_cost_under_budget_with_a_common_word_selected_on_a_5k_line_code_document() {
    let mut app = app_for(&build_5k_line_rust_fixture(), "/x/big.rs");
    settle_highlights(&mut app);

    // `settle_highlights` leaves the caret at end-of-buffer: page back to
    // the top, walk down onto the first `let` line and across to the word,
    // then select it — all through the real update seam.
    let mut pages = 0usize;
    while app.active_doc().cursors().primary().position.get() > 0 {
        press(&mut app, KeyCode::PageUp);
        pages += 1;
        assert!(pages <= 500, "the caret never paged back to the top");
    }
    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    for _ in 0..4 {
        press(&mut app, KeyCode::Right);
    }
    for _ in 0..3 {
        press_with(&mut app, KeyCode::Right, SHIFT);
    }

    let doc = app.active_doc();
    let (from, to) = doc.cursors().primary().selection_range();
    assert_eq!(
        doc.buffer().slice(from.get(), to.get()),
        Some("let"),
        "the key walk must have selected exactly `let`, or this gate measures nothing"
    );

    let per_frame = average_frame_cost(&app);

    assert!(
        per_frame < CODE_FRAME_BUDGET,
        "average render::build_rows cost with a common word selected in a 5k-line code \
         document was {:.3} ms over {RENDER_FRAMES} frames (budget: {:.3} ms)",
        per_frame.as_secs_f64() * 1_000.0,
        CODE_FRAME_BUDGET.as_secs_f64() * 1_000.0
    );
}
