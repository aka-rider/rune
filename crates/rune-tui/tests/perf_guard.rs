//! WP16 keystroke-latency performance guard: asserts the SYNCHRONOUS
//! per-keystroke cost — one `app::update` call plus the settle-step
//! `App::sync_view` the real runtime loop runs once per message batch,
//! never any spawned `Cmd`'s own off-thread work (a highlight parse, a
//! save) — stays bounded on a large document.
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
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

/// A large (~5,000-line) Rust source fixture — `DocumentKind::Code("rust")`
/// via its `.rs` path, so this exercises the SAME whole-document highlight-
/// scheduling path (plan WP16.S2/S3) a large open code file would in
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
/// must be far cheaper per keystroke since WP16 exists specifically to make
/// most of that pipeline a memoized no-op on an ordinary keystroke.
const PER_KEYSTROKE_BUDGET: Duration = Duration::from_millis(20);
const KEYSTROKES: usize = 100;

#[ignore = "This is a wall-clock bound that must ONLY run via the explicit \
            release invocation in Make (rust-perf-guard). It is inherently \
            flaky inside ordinary parallel debug `cargo test` and is marked \
            #[ignore] for that reason."]
#[test]
fn keystroke_view_cost_under_budget_on_a_5k_line_code_document() {
    let content = build_5k_line_rust_fixture();
    let mut app = App::new(
        Buffer::new(&content),
        Some(PathBuf::from("/x/big.rs")),
        Arc::new(Mem::new()),
        None,
    );
    app.doc_mut(app.active)
        .expect("doc")
        .viewport
        .set_size(120, 40);
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
