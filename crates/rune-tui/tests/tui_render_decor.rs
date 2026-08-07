//! WP4.S5: line-decoration render/caret/mouse coverage — the heading icon /
//! list bullet / quote bar / hr rule prefix `render::decor` builds, the
//! caret's decor-shifted `visual_col` (`render::overlay::apply_cursor_
//! overlays`), and the mouse decor-cell fallback (`commands::mouse::
//! offset_at`). Sits beside the basic conceal/styling render
//! tests rather than growing that file past its own budget discussion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_render_common;

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_syntax::scope::scope_table;
use rune_tui::app::{App, update};
use rune_tui::pointer::{ManualClock, MouseButton, MouseInput, MouseKind};
use rune_tui::render;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

use tui_render_common::app_for;

/// Builds `app` and its first synced view, then hands back `render::
/// build_rows`' own `Cell` grid — the same grid `render::draw`/`blit`
/// consume, so a decor assertion here is pinned against exactly what a
/// real frame would show, not a hand-rolled approximation of it.
fn rows_for(content: &str, cursor_offset: usize, focused: bool) -> Vec<Vec<render::Cell>> {
    let app = app_for(content, cursor_offset, focused);
    let view = app.active_doc().view.as_ref().expect("synced view");
    render::build_rows(&app, app.active_doc(), Some(app.active), view)
}

/// (a) A concealed `# h` row's own decor prefix carries `buf_offset == -1`
/// and the heading's own style (plan A3: the Rendered branch styles the
/// whole line uniformly, `render::decor::decor_row_cells`'s docs — the
/// icon piece's scope IS `markup.heading.N`, not a separate "icon" scope).
#[test]
fn concealed_heading_row_starts_with_a_styled_decorative_icon() {
    let content = "# Heading\n\ntext\n";
    let cursor = content.find("text").expect("fixture has a tail line");
    let rows = rows_for(content, cursor, true);
    let row = rows.first().expect("at least one row");

    let heading1 = scope_table()
        .resolve("markup.heading.1")
        .expect("registered scope");
    let app = app_for(content, cursor, true);
    let expected_style = app.theme.scope_style(heading1);

    let icon_cells: Vec<_> = row.iter().take_while(|c| c.buf_offset < 0).collect();
    assert!(
        !icon_cells.is_empty(),
        "expected at least one decorative icon cell before the real content:\n{row:?}"
    );
    for cell in &icon_cells {
        assert_eq!(cell.buf_offset, -1);
        assert_eq!(cell.style, expected_style);
    }

    // The first REAL cell must map to "Heading"'s own first byte (the `#
    // `/`## ` marker stays hidden, byte-neutral — plan Gotchas).
    let first_real = row
        .iter()
        .find(|c| c.buf_offset >= 0)
        .expect("some content cell");
    assert_eq!(
        first_real.buf_offset as usize,
        content.find("Heading").unwrap()
    );
}

/// (b) A concealed blockquote line shows its own bar decor, styled
/// `markup.quote.marker` — the quoted TEXT itself keeps its plain scope
/// (plan A4: "only the bar is added").
#[test]
fn concealed_quote_row_shows_its_bar_decor() {
    let content = "> quote\n\ntext\n";
    let cursor = content.find("text").expect("fixture has a tail line");
    let rows = rows_for(content, cursor, true);
    let row = rows.first().expect("at least one row");

    let marker_scope = scope_table()
        .resolve("markup.quote.marker")
        .expect("registered scope");
    let app = app_for(content, cursor, true);
    let expected_style = app.theme.scope_style(marker_scope);

    let bar = row.first().expect("row has at least the bar cell");
    assert_eq!(bar.buf_offset, -1);
    assert_eq!(bar.style, expected_style);
    assert_eq!(bar.text, app.icons().quote_bar);
}

/// (c) A `---` thematic break renders as a full-width rule row — every cell
/// decorative (plan WP3.S3's rule exemption: it always attaches, clamped to
/// the available width, since it has no competing content).
#[test]
fn thematic_break_renders_a_full_width_rule_row() {
    let content = "---\n\ntext\n";
    let cursor = content.find("text").expect("fixture has a tail line");
    let rows = rows_for(content, cursor, true);
    let row = rows.first().expect("at least one row");

    assert!(!row.is_empty(), "expected a non-empty rule row");
    let total_width: usize = row.iter().map(|c| c.width as usize).sum();
    assert!(
        row.iter().all(|c| c.buf_offset < 0),
        "every cell of an hr rule row must be decorative:\n{row:?}"
    );
    // The doc's own wrap width (`tui_render_common::app_for` sets
    // `WIDTH`/`HEIGHT - 1`) is what the rule is clamped to.
    assert_eq!(total_width, tui_render_common::WIDTH as usize);
}

/// (d) An ordered list item's decor carries the user's OWN marker text
/// verbatim (plan WP2.S5: "ordered -> trimmed `content[item.marker]` +
/// space") rather than a synthesised bullet glyph.
#[test]
fn ordered_list_item_decor_shows_its_own_number() {
    let content = "1. item\n\ntext\n";
    let cursor = content.find("text").expect("fixture has a tail line");
    let rows = rows_for(content, cursor, true);
    let row = rows.first().expect("at least one row");

    let decor_text: String = row
        .iter()
        .take_while(|c| c.buf_offset < 0)
        .map(|c| c.text.clone())
        .collect();
    assert_eq!(decor_text, "1. ");

    let first_real = row
        .iter()
        .find(|c| c.buf_offset >= 0)
        .expect("some content cell");
    assert_eq!(
        first_real.buf_offset as usize,
        content.find("item").unwrap()
    );
}

/// (e) Clicking a decor cell (WP4.S4) places the caret at the line's own
/// content start — NOT document offset 0 (the old, untested `Some(0)`
/// fallback the plan Gotchas call out as a bug: a click on a decorative
/// cell used to silently jump to the document's very first byte).
#[test]
fn click_on_a_decor_cell_places_the_caret_at_the_lines_content_start() {
    let content = "## Heading\n\ntext\n";
    let cursor = content.find("text").expect("fixture has a tail line");
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.pointer_clock = Box::new(ManualClock::new());
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor);
    app.doc_mut(id).unwrap().viewport.set_size(40, 10);
    app.frame_width = 40;
    app.frame_height = 11; // + footer row
    app.sync_view();

    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let editor = rune_tui::layout::geometry(area, &app).editor;

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: editor.x, // column 0 of the editor content: the decor icon
            row: editor.y,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );

    assert_eq!(
        app.active_doc().cursors.primary().position,
        content.find("Heading").expect("fixture has a heading"),
        "a click on the heading's own icon cell must land on the heading text's first byte"
    );
}

/// (f) `[critic R4]` case (a): an unfocused pane forces every reveal-Decide
/// element concealed regardless of cursor position, so a heading hosting
/// the cursor still renders its decor (icon + uniform heading style) even
/// though `Document::has_insertion_point` (`focused && !is_read_only()`) keeps the caret
/// itself unpainted — extends the existing
/// `concealed_heading_marker_not_visible_when_unfocused_even_with_cursor_
/// on_it`. The decor-shift arithmetic `apply_cursor_overlays` runs for
/// EVERY cursor unconditionally once overlays are shown; here it never
/// executes at all (the `show_overlays` gate returns first), which is
/// exactly why no caret glyph can ever land on a decorated cell in this
/// case — the two states (decor rendered, caret withheld) are independent
/// facts this test pins together.
#[test]
fn unfocused_pane_with_cursor_on_a_decorated_heading_still_renders_its_icon() {
    let content = "## Heading\n";
    let rows = rows_for(content, 0, false); // cursor ON the heading line, unfocused
    let row = rows.first().expect("at least one row");

    let heading2 = scope_table()
        .resolve("markup.heading.2")
        .expect("registered scope");
    let app = app_for(content, 0, false);
    let expected_style = app.theme.scope_style(heading2);

    let icon_cells: Vec<_> = row.iter().take_while(|c| c.buf_offset < 0).collect();
    assert!(
        !icon_cells.is_empty(),
        "an unfocused pane must still render the heading's decor:\n{row:?}"
    );
    for cell in &icon_cells {
        assert_eq!(cell.style, expected_style);
    }

    let buf = tui_render_common::render_to_test_backend(&app);
    assert_eq!(
        tui_render_common::caret_column(
            &buf,
            tui_render_common::EDITOR_TOP_ROW,
            tui_render_common::WIDTH
        ),
        None,
        "an unfocused pane must still show no caret, decor or not"
    );
}

/// (g) `[critic R4]` case (b): a list item's marker line keeps its own
/// decor (the bullet) even while the cursor sits on a LATER source line
/// that lazily continues the same item's paragraph — `ListItemM.line` (the
/// Decide policy's cursor-presence check) is keyed to the item's OWN first
/// physical line only (`parse::block::build_list_items`), never to any
/// line the item's content happens to continue onto. The continuation
/// line's own rows carry no decor at all (only the marker's line ever
/// does) and the caret lands there at the correct, un-shifted column,
/// proving the two rows' decor states never bleed into each other.
#[test]
fn wrapped_list_item_keeps_its_bullet_decor_with_the_caret_on_a_continuation_line() {
    let content = "- first line of the item that is reasonably long here\n\
continuation text on the second source line here\n\n\
tail paragraph\n";
    let cursor = content
        .find("continuation")
        .expect("fixture has a continuation line");

    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor);
    app.doc_mut(id).unwrap().viewport.set_size(20, 20);
    app.sync_view();

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(&app, app.active_doc(), Some(app.active), view);

    let list_scope = scope_table()
        .resolve("markup.list")
        .expect("registered scope");
    let expected_style = app.theme.scope_style(list_scope);

    // The item's marker line wraps across several rows (narrow width);
    // every one of them keeps the bullet/continuation-blank decor.
    let marker_rows: Vec<&Vec<render::Cell>> = rows
        .iter()
        .take_while(|row| row.iter().any(|c| c.buf_offset < 0))
        .collect();
    assert!(
        marker_rows.len() >= 2,
        "expected the item's first source line to wrap into multiple decorated rows:\n{rows:?}"
    );
    let bullet = marker_rows[0].first().expect("bullet cell");
    assert_eq!(bullet.buf_offset, -1);
    assert_eq!(bullet.style, expected_style);

    // The continuation source line's own rows carry NO decor at all, and
    // the caret lands on the FIRST cell of the row containing the cursor's
    // own byte — un-shifted, since that row's decor width is zero.
    let caret_row = rows
        .iter()
        .find(|row| {
            row.iter().any(|c| {
                c.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::REVERSED)
            })
        })
        .expect("the caret must land on some row");
    assert!(
        caret_row.iter().all(|c| c.buf_offset != -1),
        "the continuation line's own row must carry no decor prefix:\n{caret_row:?}"
    );
    let caret_cell = caret_row
        .iter()
        .find(|c| {
            c.style
                .add_modifier
                .contains(ratatui::style::Modifier::REVERSED)
        })
        .expect("caret cell");
    assert_eq!(
        caret_cell.buf_offset as usize, cursor,
        "the caret must land exactly on the cursor's own byte offset"
    );
}
