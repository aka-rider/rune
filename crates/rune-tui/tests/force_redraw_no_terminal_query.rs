//! `Terminal::clear` round-trips a cursor-position query through
//! the real terminal, which deadlocks against `spawn_input_reader`'s own
//! read of that same connection. `Guard::force_redraw` must never reach
//! `Terminal::clear` again — this is verified by a dedicated test.
//!
//! The replacement, `Terminal::resize`, is query-free only under
//! `Viewport::Fullscreen` (see `ratatui_core_resize_resets_previous_buffer`
//! for the buffer-reset half of that claim). `Guard::new` gets
//! `Viewport::Fullscreen` by calling `RtTerminal::new`, never
//! `with_options`/`TerminalOptions` — this test pins that too, since
//! switching constructors would silently reintroduce the query.
#![allow(clippy::unwrap_used, clippy::expect_used)]

fn term_rs_contents() -> String {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let term_rs = manifest_dir.join("src").join("term.rs");
    std::fs::read_to_string(&term_rs).expect("read src/term.rs")
}

#[test]
fn force_redraw_never_calls_terminal_clear() {
    let contents = term_rs_contents();
    assert!(
        !contents.contains(".terminal.clear()"),
        "Guard must never call Terminal::clear — it round-trips a cursor \
         query that deadlocks against the input-reader thread"
    );
}

#[test]
fn guard_terminal_stays_fullscreen_viewport() {
    let contents = term_rs_contents();
    assert!(
        contents.contains("RtTerminal::new("),
        "Guard::new must keep constructing its Terminal with RtTerminal::new \
         (the Viewport::Fullscreen constructor)"
    );
    assert!(
        !contents.contains("with_options") && !contents.contains("TerminalOptions"),
        "Guard must never switch to a non-Fullscreen viewport — \
         Terminal::resize is query-free only under Viewport::Fullscreen"
    );
}
