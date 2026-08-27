//! `TestBackend` integration tests for the center pane's
//! title row and breadcrumb (`render::draw` delegates to `title::draw` and
//! `breadcrumb::overlay`): the title now
//! lives at row 1 (row 0 is the center block's top border), and the
//! breadcrumb is spliced onto the block's own bottom border row, not a
//! separate reserved row). Mirrors `tests/chrome.rs`'s pattern: drive the
//! real `App`/`render::draw`, no runtime loop, no real terminal.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
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
    // Seeds through the real geometry chokepoint (plan gotcha 9) rather
    // than a bare `viewport.set_size` — `App::relayout` (run inside
    // `sync_view`) sizes the viewport from the SAME `layout::geometry`
    // `render::draw` itself reads, so the two can never disagree.
    app.frame = Some(rune_tui::app::FrameSize::new(WIDTH, HEIGHT));
    app.sync_view();
    app
}

fn row_text(app: &App, y: u16, width: u16) -> String {
    row_text_sized(app, y, width, HEIGHT)
}

fn row_text_sized(app: &App, y: u16, width: u16, height: u16) -> String {
    testgrid::row(app, y, width, height)
}

/// Row 1 of the center pane (row 0 is the top border) shows
/// the active document's display name.
#[test]
fn title_row_shows_the_active_doc_name() {
    let app = app_for("hello", Some("/notes/todo.md"));
    let title_row = row_text(&app, 1, WIDTH);
    assert!(
        title_row.contains("todo.md"),
        "expected the doc name on the title row:\n{title_row}"
    );
}

/// A pathless (untitled) document's title row falls back to "[No Name]"
/// (`Document::file_name`'s own default).
#[test]
fn title_row_shows_no_name_placeholder_when_pathless() {
    let app = app_for("hello", None);
    let title_row = row_text(&app, 1, WIDTH);
    assert!(
        title_row.contains("[No Name]"),
        "expected the '[No Name]' placeholder on the title row:\n{title_row}"
    );
}

/// The default no-arg launch's own document (`App::new_untitled`) shows
/// "Untitled 1" on the title row, not the generic "[No Name]" placeholder
/// every OTHER pathless document falls back to.
#[test]
fn title_row_shows_untitled_1_for_the_default_untitled_document() {
    let mut app = App::new_untitled(Arc::new(Mem::new()), None);
    app.frame = Some(rune_tui::app::FrameSize::new(WIDTH, HEIGHT));
    app.sync_view();

    let title_row = row_text(&app, 1, WIDTH);
    assert!(
        title_row.contains("Untitled 1"),
        "expected \"Untitled 1\" on the title row:\n{title_row}"
    );
}

/// Focused, the title shows the WHOLE file name, extension
/// included — before, the field held only the stem while editing, so a
/// focused row never showed `.md` at all.
#[test]
fn focused_title_row_shows_the_extension_too() {
    let mut app = app_for("hello", Some("/notes/todo.md"));
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('r'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );
    app.sync_view();

    let title_row = row_text(&app, 1, WIDTH);
    assert!(
        title_row.contains("todo.md"),
        "the focused title must show the extension too:\n{title_row}"
    );
}

/// The dirty dot appears on the title row after a real edit is driven
/// through `app::update` ("drive a real key Msg through
/// update"; the disappears-on-save case is deliberately NOT covered here —
/// materializing a save is heavier machinery than this test needs).
#[test]
fn dirty_dot_appears_after_an_edit() {
    let mut app = app_for("hello", Some("/notes/todo.md"));

    let clean_row = row_text(&app, 1, WIDTH);
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

    let dirty_row = row_text(&app, 1, WIDTH);
    assert!(
        dirty_row.contains('\u{2022}'),
        "expected the dirty dot on the title row after an edit:\n{dirty_row}"
    );
}

/// The center block's bottom border row (`HEIGHT - 2`: `HEIGHT - 1` is the
/// footer) carries the breadcrumb's path segments, spliced onto the
/// border by `breadcrumb::overlay` — no longer its own
/// reserved row.
#[test]
fn breadcrumb_row_shows_path_segments_for_a_file_backed_doc() {
    let app = app_for("hello", Some("/notes/vault/todo.md"));
    let breadcrumb_row = row_text(&app, HEIGHT - 2, WIDTH);
    assert!(
        breadcrumb_row.contains("notes/vault"),
        "expected the directory chain joined by a bare slash:\n{breadcrumb_row}"
    );
    assert!(
        breadcrumb_row.contains("vault › todo.md"),
        "expected the leaf set off from its directory by ' › ':\n{breadcrumb_row}"
    );
    // Compares against the FULL width, not `trim_end()`'s trimmed one — a
    // trimmed comparison would pass even if the border stopped one (or
    // more) columns short of the frame's actual right edge, trailing blank
    // columns and all. This is exactly the "blank last column" defect
    // class (see `chrome.rs`'s dedicated test and `layout.rs`'s own
    // `assert_invariant` checks): the row's LAST character, at index
    // `WIDTH - 1`, must be the border corner.
    assert_eq!(
        breadcrumb_row.chars().count(),
        WIDTH as usize,
        "expected the breadcrumb row to span the full frame width with no short tail:\n{breadcrumb_row:?}"
    );
    assert!(
        breadcrumb_row.ends_with("──╯"),
        "expected the breadcrumb row to end in the border's bottom-right corner ON THE LAST COLUMN:\n{breadcrumb_row}"
    );
}

/// A pathless document's bottom border row is left exactly as
/// `render::draw`'s `Block` already painted it — plain dash fill and
/// corners, no path text spliced in (`breadcrumb::overlay`'s early return
/// "renders nothing" for a pathless doc, but "nothing" now
/// means "don't touch the border row" rather than "leave a blank row").
#[test]
fn pathless_doc_has_no_breadcrumb_content() {
    let app = app_for("hello", None);
    let breadcrumb_row = row_text(&app, HEIGHT - 2, WIDTH);
    // Full-width comparison (see the sibling test above for why
    // `trim_end()` would mask a short row): the border's own closing
    // corner must land ON the last column, not somewhere before it.
    assert_eq!(
        breadcrumb_row.chars().count(),
        WIDTH as usize,
        "expected the border row to span the full frame width with no short tail:\n{breadcrumb_row:?}"
    );
    assert!(
        breadcrumb_row.starts_with('╰') && breadcrumb_row.ends_with('╯'),
        "expected the plain border row (no crumb spliced in):\n{breadcrumb_row:?}"
    );
    assert!(
        !breadcrumb_row.contains("hello"),
        "expected no path text on the border row for a pathless doc:\n{breadcrumb_row}"
    );
}

/// A center pane too short for even a 1-cell-per-side border
/// (`center.height < 3`) falls back to the UNBORDERED
/// layout: no border glyphs anywhere in the frame, and — above all — no
/// panic. The title still renders (its own gate is just `content.height
/// >= 1`, independent of the border), just without a border around it.
#[test]
fn tiny_terminal_falls_back_to_the_unbordered_layout_without_panicking() {
    let app = app_for("hello", Some("/notes/todo.md"));
    // main area = 3 - 1 (footer) = 2 rows: center.height == 2 < 3, so
    // `layout::geometry` reports `center_bordered == false`.
    let row0 = row_text_sized(&app, 0, WIDTH, 3);
    assert!(
        !row0.contains('╭') && !row0.contains('╮'),
        "no border corners expected in the unbordered fallback:\n{row0}"
    );
    assert!(
        row0.contains("todo.md"),
        "the title still renders (unbordered) at content row 0:\n{row0}"
    );
}
