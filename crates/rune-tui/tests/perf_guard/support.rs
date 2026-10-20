#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;
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
pub const VIEWPORT: (u16, u16) = (120, 40);

pub const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

/// A large (~5,000-line) Rust source fixture — `DocumentKind::Code("rust")`
/// via its `.rs` path, so this exercises the SAME whole-document highlight-
/// scheduling path a large open code file would in
/// practice, not just the markdown display pipeline `rune-md`'s own guard
/// already covers.
pub fn build_5k_line_rust_fixture() -> String {
    let mut src = String::with_capacity(5_000 * 40);
    src.push_str("//! A large synthetic Rust fixture for the keystroke perf guard.\n\n");
    for i in 0..1_000 {
        src.push_str(&format!(
            "fn function_{i}(a: i32, b: i32) -> i32 {{\n    let sum = a + b;\n    sum * 2\n}}\n\n"
        ));
    }
    src
}

pub fn type_one_char_at_end(app: &mut App, ch: char, effects: &mut Effects) {
    let id = app.active;
    let end = app.doc(id).expect("doc").buffer().content().len();
    app.doc_mut(id)
        .expect("doc")
        .set_cursors(rune_core::cursor::CursorSet::new(end));
    app::update(
        app,
        Msg::Key(KeyInput {
            code: KeyCode::Char(ch),
            mods: Mods::NONE,
        }),
        effects,
    );
}

/// A markdown fixture with 200 short ```` ```rust ```` fences separated by
/// prose — roughly 2,000 lines, four or five fences inside any one 40-row
/// window. Many regions AND several visible ones at once is the shape the
/// per-frame render path scales with: `highlight::visible_spans` walks every
/// region of the document and runs a tree-sitter range query for each one
/// that intersects the window.
pub fn build_fenced_markdown_fixture() -> String {
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

pub fn app_for(content: &str, path: &str) -> App {
    let vfs = Arc::new(Mem::new());
    let launch = rune_tui::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new(path))
        .expect("the launch path resolves");
    let mut app = App::new(Buffer::new(content), Some(launch), vfs, None);
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
pub fn settle_highlights(app: &mut App) {
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
pub fn average_frame_cost(app: &App) -> Duration {
    let view = app
        .active_doc()
        .view
        .as_ref()
        .expect("sync_view must have cached a view");
    // One untimed frame first: the same settle-before-timing discipline the
    // keystroke guard uses, so a first-paint-only cost is never what the
    // budget is compared against.
    std::hint::black_box(render::build_rows(app, render::RowSource::Shown, view));
    let start = Instant::now();
    for _ in 0..RENDER_FRAMES {
        std::hint::black_box(render::build_rows(app, render::RowSource::Shown, view));
    }
    start.elapsed() / u32::try_from(RENDER_FRAMES).unwrap_or(1)
}

pub const RENDER_FRAMES: usize = 200;

/// A 2 MiB single-paragraph markdown document — well over `runtime::
/// bootstrap`'s large-document threshold (1 MiB), so a real bootstrap would
/// defer its display-pipeline compute to a background `Cmd` and leave
/// `Document::view` `None` until that reply lands.
pub fn build_2mb_prose_fixture() -> String {
    let line = "The quick brown fox jumps over the lazy dog near the riverbank at dawn.\n";
    let mut doc = String::with_capacity(2_100_000);
    while doc.len() < 2_097_152 {
        doc.push_str(line);
    }
    doc
}

pub fn press(app: &mut App, code: KeyCode) {
    press_with(app, code, Mods::NONE);
}

pub fn press_with(app: &mut App, code: KeyCode, mods: Mods) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(KeyInput { code, mods }), &mut effects);
    effects.cmds.clear();
    app.sync_view();
}
