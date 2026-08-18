//! Keystroke-latency performance guard: asserts the SYNCHRONOUS
//! per-keystroke cost — one `app::update` call plus the settle-step
//! `App::sync_view` the real runtime loop runs once per message batch,
//! never any spawned `Cmd`'s own off-thread work (a highlight parse, a
//! save) — stays bounded on a large document.
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
//! debug `cargo test`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::render;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::Mem;

/// The editor viewport every gate below measures against — an ordinary
/// full-screen terminal, so the visible window is a realistic slice of the
/// document rather than the whole of it.
const VIEWPORT: (u16, u16) = (120, 40);

/// A large (~5,000-line) Rust source fixture — `DocumentKind::Code("rust")`
/// via its `.rs` path, so this exercises the SAME whole-document highlight-
/// scheduling path a large open code file would in
/// practice, not just the markdown display pipeline `rune-md`'s own guard
/// already covers.
fn build_5k_line_rust_fixture() -> String {
    let mut src = String::with_capacity(5_000 * 40);
    src.push_str("//! A large synthetic Rust fixture for the keystroke perf guard.\n\n");
    for i in 0..1_000 {
        src.push_str(&format!(
            "fn function_{i}(a: i32, b: i32) -> i32 {{\n    let sum = a + b;\n    sum * 2\n}}\n\n"
        ));
    }
    src
}

fn type_one_char_at_end(app: &mut App, ch: char, effects: &mut Effects) {
    let id = app.active;
    let end = app.doc(id).expect("doc").buffer.content().len();
    app.doc_mut(id).expect("doc").cursors = rune_core::cursor::CursorSet::new(end);
    app::update(
        app,
        Msg::Key(KeyInput {
            code: KeyCode::Char(ch),
            mods: Mods::NONE,
        }),
        effects,
    );
}

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
            flaky inside ordinary parallel debug `cargo test` and is marked \
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

/// A markdown fixture with 200 short ```` ```rust ```` fences separated by
/// prose — roughly 2,000 lines, four or five fences inside any one 40-row
/// window. Many regions AND several visible ones at once is the shape the
/// per-frame render path scales with: `highlight::visible_spans` walks every
/// region of the document and runs a tree-sitter range query for each one
/// that intersects the window.
fn build_fenced_markdown_fixture() -> String {
    let mut src = String::with_capacity(200 * 200);
    src.push_str("# A large synthetic markdown fixture for the render perf guard.\n\n");
    for i in 0..200 {
        src.push_str(&format!(
            "Prose paragraph {i} introducing the snippet below.\n\n\
             ```rust\n\
             fn function_{i}(a: i32, b: i32) -> i32 {{\n    \
             let sum = a + b;\n    sum * 2\n}}\n\
             ```\n\n"
        ));
    }
    src
}

fn app_for(content: &str, path: &str) -> App {
    let mut app = App::new(
        Buffer::new(content),
        Some(PathBuf::from(path)),
        Arc::new(Mem::new()),
        None,
    );
    app.doc_mut(app.active)
        .expect("doc")
        .viewport
        .set_size(VIEWPORT.0, VIEWPORT.1);
    app
}

/// Drives the active document's highlighting to completion through the real
/// message path: type one character to schedule, run the `Cmd` inline,
/// deliver its reply, repeat until nothing further is scheduled.
///
/// Load-bearing for what follows: the per-frame query only reaches
/// `rune_ts::highlight_range` for a region that actually holds a retained
/// tree, so timing an unhighlighted document would measure a path no real
/// session spends any time on. Only `CmdKind::Highlight` is run — a snapshot
/// or save `Cmd` is off-thread work in production and out of scope here.
fn settle_highlights(app: &mut App) {
    app.sync_view();
    let mut effects = Effects::default();
    type_one_char_at_end(app, '!', &mut effects);
    // A reply can arm `pending` and schedule one more round; a handful of
    // rounds is far more than the one or two that actually occur, and the
    // bound means a scheduling bug fails this guard rather than hanging it.
    for _ in 0..8 {
        let cmds: Vec<_> = effects
            .cmds
            .drain(..)
            .filter(|cmd| cmd.kind() == CmdKind::Highlight)
            .collect();
        if cmds.is_empty() {
            break;
        }
        for cmd in cmds {
            if let Some(msg) = cmd.run() {
                app::update(app, msg, &mut effects);
            }
        }
    }
    effects.cmds.clear();
    app.sync_view();
}

/// The average cost of one `render::build_rows` call over the settled view —
/// exactly what `render::draw` does once per frame, and nothing else.
fn average_frame_cost(app: &App) -> Duration {
    let view = app
        .active_doc()
        .view
        .as_ref()
        .expect("sync_view must have cached a view");
    // One untimed frame first: the same settle-before-timing discipline the
    // keystroke guard uses, so a first-paint-only cost is never what the
    // budget is compared against.
    std::hint::black_box(render::build_rows(
        app,
        app.active_doc(),
        Some(app.active),
        view,
    ));
    let start = Instant::now();
    for _ in 0..RENDER_FRAMES {
        std::hint::black_box(render::build_rows(
            app,
            app.active_doc(),
            Some(app.active),
            view,
        ));
    }
    start.elapsed() / u32::try_from(RENDER_FRAMES).unwrap_or(1)
}

const RENDER_FRAMES: usize = 200;

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
            flaky inside ordinary parallel debug `cargo test` and is marked \
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
            flaky inside ordinary parallel debug `cargo test` and is marked \
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

/// A 2 MiB single-paragraph markdown document — well over `runtime::
/// bootstrap`'s large-document threshold (1 MiB), so a real bootstrap would
/// defer its display-pipeline compute to a background `Cmd` and leave
/// `Document::view` `None` until that reply lands.
fn build_2mb_prose_fixture() -> String {
    let line = "The quick brown fox jumps over the lazy dog near the riverbank at dawn.\n";
    let mut doc = String::with_capacity(2_100_000);
    while doc.len() < 2_097_152 {
        doc.push_str(line);
    }
    doc
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
            flaky inside ordinary parallel debug `cargo test` and is marked \
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
