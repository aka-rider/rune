//! WP6.S2 done-when: `TestBackend` integration tests for the center pane's
//! reserved title/breadcrumb rows (`render::draw` delegates to
//! `title::draw`/`breadcrumb::draw` — plan WP6.S1/S2). Mirrors
//! `tests/chrome.rs`'s pattern: drive the real `App`/`render::draw`, no
//! runtime loop, no real terminal.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer as RtBuffer;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::render;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_for(content: &str, path: Option<&str>) -> App {
    let mut app = App::new(
        Buffer::new(content),
        path.map(PathBuf::from),
        Arc::new(Mem::new()),
        None,
    );
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn draw(app: &App) -> RtBuffer {
    draw_sized(app, WIDTH, HEIGHT)
}

fn draw_sized(app: &App, width: u16, height: u16) -> RtBuffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal construction");
    terminal
        .draw(|frame| render::draw(app, frame))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn row_text(buf: &RtBuffer, y: u16, width: u16) -> String {
    let mut s = String::new();
    for x in 0..width {
        if let Some(cell) = buf.cell((x, y)) {
            s.push_str(cell.symbol());
        }
    }
    s
}

/// Row 0 of the center pane (no left pane in these fixtures) shows the
/// active document's display name.
#[test]
fn title_row_shows_the_active_doc_name() {
    let app = app_for("hello", Some("/notes/todo.md"));
    let buf = draw(&app);
    let title_row = row_text(&buf, 0, WIDTH);
    assert!(
        title_row.contains("todo.md"),
        "expected the doc name on the title row:\n{title_row}"
    );
}

/// A pathless (untitled) document's title row falls back to "[No Name]"
/// (`Document::file_name`'s Go-parity default — WP1).
#[test]
fn title_row_shows_no_name_placeholder_when_pathless() {
    let app = app_for("hello", None);
    let buf = draw(&app);
    let title_row = row_text(&buf, 0, WIDTH);
    assert!(
        title_row.contains("[No Name]"),
        "expected the '[No Name]' placeholder on the title row:\n{title_row}"
    );
}

/// The dirty dot appears on the title row after a real edit is driven
/// through `app::update` (plan WP6.S2: "drive a real key Msg through
/// update"; the disappears-on-save case is deliberately NOT covered here —
/// materializing a save is heavier machinery than this test needs).
#[test]
fn dirty_dot_appears_after_an_edit() {
    let mut app = app_for("hello", Some("/notes/todo.md"));

    let clean_row = row_text(&draw(&app), 0, WIDTH);
    assert!(
        !clean_row.contains('\u{2022}'),
        "a freshly opened doc must not show the dirty dot:\n{clean_row}"
    );

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('!'),
            mods: Mods::NONE,
        }),
        &mut effects,
    );
    app.sync_view();

    let dirty_row = row_text(&draw(&app), 0, WIDTH);
    assert!(
        dirty_row.contains('\u{2022}'),
        "expected the dirty dot on the title row after an edit:\n{dirty_row}"
    );
}

/// Row 1 of the center pane shows the file-backed doc's path segments.
#[test]
fn breadcrumb_row_shows_path_segments_for_a_file_backed_doc() {
    let app = app_for("hello", Some("/notes/vault/todo.md"));
    let buf = draw(&app);
    let breadcrumb_row = row_text(&buf, 1, WIDTH);
    assert!(
        breadcrumb_row.contains("notes"),
        "expected a path segment on the breadcrumb row:\n{breadcrumb_row}"
    );
    assert!(
        breadcrumb_row.contains("vault"),
        "expected a path segment on the breadcrumb row:\n{breadcrumb_row}"
    );
    assert!(
        breadcrumb_row.contains("todo.md"),
        "expected the file name segment on the breadcrumb row:\n{breadcrumb_row}"
    );
}

/// A pathless document renders no breadcrumb content — row 1 stays blank
/// (plan WP6.S1: "pathless docs render nothing").
#[test]
fn pathless_doc_has_no_breadcrumb_content() {
    let app = app_for("hello", None);
    let buf = draw(&app);
    let breadcrumb_row = row_text(&buf, 1, WIDTH);
    assert_eq!(
        breadcrumb_row.trim(),
        "",
        "expected an empty breadcrumb row for a pathless doc:\n{breadcrumb_row:?}"
    );
}

/// A tiny terminal (center pane 3 rows: 4 total minus the 1-row footer)
/// drops BOTH the title and breadcrumb rows rather than leaving one
/// underneath a 1- or 2-row editor sliver (plan WP6.S2: "only when the
/// center pane is >= 4 rows tall") — and, above all, must not panic.
#[test]
fn tiny_terminal_drops_both_chrome_rows_without_panicking() {
    let app = app_for("hello", Some("/notes/todo.md"));
    let buf = draw_sized(&app, WIDTH, 4); // main area = 4 - 1 (footer) = 3 rows

    let row0 = row_text(&buf, 0, WIDTH);
    assert!(
        !row0.contains("todo.md"),
        "3-row center pane must not show the title row:\n{row0}"
    );
    let row1 = row_text(&buf, 1, WIDTH);
    assert!(
        !row1.contains("notes"),
        "3-row center pane must not show the breadcrumb row:\n{row1}"
    );
}
