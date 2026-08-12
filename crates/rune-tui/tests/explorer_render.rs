//! Row visual language render coverage (`CLAUDE.md`'s "Row visual
//! language" section): the Explorer's directory/file hue split, its icon
//! column, and both left-column panes' cursor/active-document background
//! bars — the Explorer's first rendering coverage of any kind.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod explorer_common;

use std::sync::Arc;

use ratatui::style::Modifier;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::pane::Pane;
use rune_tui::runtime::Effects;
use rune_tui::testgrid;
use rune_tui::theme::icons::IconTier;
use rune_vfs::Mem;

use explorer_common::{app_with, load_explorer, seeded_vfs};

const WIDTH: u16 = 40;
const HEIGHT: u16 = 24;

fn explorer_inner(app: &App) -> ratatui::layout::Rect {
    let area = ratatui::layout::Rect::new(0, 0, WIDTH, HEIGHT);
    rune_tui::layout::geometry(area, app).explorer_inner
}

#[test]
fn a_directory_row_is_bold_blue_and_a_file_row_is_plain_text() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    app.icon_tier = IconTier::Unicode;
    load_explorer(&mut app);

    let dir_idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "sub")
        .expect("seeded fixture has a sub directory");
    let file_idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "a.md")
        .expect("seeded fixture has a.md");
    app.explorer.nav.cursor = app.explorer.entries.len(); // past the last row: nothing selected
    app.explorer.nav.top = 0;

    let inner = explorer_inner(&app);
    let buf = testgrid::draw(&app, WIDTH, HEIGHT);

    let dir_row = inner.y + 1 + dir_idx as u16;
    let file_row = inner.y + 1 + file_idx as u16;
    // "  " prefix (2 cells) then the name starts at column 2 on the
    // Unicode tier (no icon column).
    let dir_cell = buf.cell((inner.x + 2, dir_row)).expect("dir cell");
    let file_cell = buf.cell((inner.x + 2, file_row)).expect("file cell");

    assert_ne!(
        app.theme.chrome.dir_normal.fg, app.theme.chrome.file_normal.fg,
        "the two kinds must be distinguishable by hue, not only by weight"
    );
    assert_eq!(dir_cell.fg, app.theme.chrome.dir_normal.fg.unwrap());
    assert!(dir_cell.modifier.contains(Modifier::BOLD));
    assert_eq!(file_cell.fg, app.theme.chrome.file_normal.fg.unwrap());
    assert!(!file_cell.modifier.contains(Modifier::BOLD));
}

#[test]
fn the_cursor_row_bar_covers_every_column_including_past_the_name_and_neighbors_carry_none() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    app.explorer.nav.cursor = 0;
    app.explorer.nav.top = 0;
    assert_eq!(app.focus(), Pane::Explorer);

    let inner = explorer_inner(&app);
    let buf = testgrid::draw(&app, WIDTH, HEIGHT);
    let cursor_bg = app.theme.chrome.row_cursor_bg.bg.unwrap();

    let cursor_row = inner.y + 1;
    for x in inner.x..inner.x + inner.width {
        let cell = buf.cell((x, cursor_row)).expect("cursor row cell");
        assert_eq!(cell.bg, cursor_bg, "column {x} must carry the cursor bg");
    }

    let neighbor_row = cursor_row + 1;
    for x in inner.x..inner.x + inner.width {
        let cell = buf.cell((x, neighbor_row)).expect("neighbor row cell");
        assert_ne!(
            cell.bg, cursor_bg,
            "column {x} on a non-cursor row must not carry the cursor bg"
        );
    }
}

#[test]
fn an_unfocused_explorer_paints_no_bar_but_keeps_its_cursor_prefix() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    app.explorer.nav.cursor = 0;
    app.explorer.nav.top = 0;

    let mut effects = Effects::default();
    app.set_focus_pane(Pane::Editor, &mut effects);
    assert_ne!(app.focus(), Pane::Explorer);

    let inner = explorer_inner(&app);
    let buf = testgrid::draw(&app, WIDTH, HEIGHT);
    let cursor_bg = app.theme.chrome.row_cursor_bg.bg.unwrap();

    let cursor_row = inner.y + 1;
    for x in inner.x..inner.x + inner.width {
        let cell = buf.cell((x, cursor_row)).expect("cursor row cell");
        assert_ne!(cell.bg, cursor_bg, "an unfocused pane paints no bar");
    }
    let prefix = buf.cell((inner.x, cursor_row)).expect("prefix cell");
    assert_eq!(
        prefix.symbol(),
        "\u{203a}",
        "the cursor prefix stays always-on"
    );
}

#[test]
fn the_nerd_tier_inserts_a_two_cell_icon_column_the_unicode_tier_does_not() {
    let mem = seeded_vfs();

    let mut unicode_app = app_with(&mem);
    unicode_app.icon_tier = IconTier::Unicode;
    load_explorer(&mut unicode_app);
    let file_idx = unicode_app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "a.md")
        .expect("fixture has a.md");
    unicode_app.explorer.nav.cursor = unicode_app.explorer.entries.len();
    unicode_app.explorer.nav.top = 0;

    let mut nerd_app = app_with(&mem);
    nerd_app.icon_tier = IconTier::Nerd;
    load_explorer(&mut nerd_app);
    nerd_app.explorer.nav.cursor = nerd_app.explorer.entries.len();
    nerd_app.explorer.nav.top = 0;

    let inner = explorer_inner(&unicode_app);
    let row = inner.y + 1 + file_idx as u16;

    let unicode_buf = testgrid::draw(&unicode_app, WIDTH, HEIGHT);
    let nerd_buf = testgrid::draw(&nerd_app, WIDTH, HEIGHT);

    // Unicode tier: name starts right after the 2-cell prefix.
    let unicode_name_start = unicode_buf
        .cell((inner.x + 2, row))
        .expect("unicode name cell");
    assert_eq!(unicode_name_start.symbol(), "a");

    // Nerd tier: a 2-cell icon column (glyph + space) sits between the
    // prefix and the name, so the name starts two columns later than on
    // the Unicode tier.
    let nerd_icon = nerd_buf.cell((inner.x + 2, row)).expect("nerd icon cell");
    assert_ne!(
        nerd_icon.symbol(),
        "a",
        "an icon glyph occupies this column"
    );
    let nerd_name_start = nerd_buf
        .cell((inner.x + 4, row))
        .expect("nerd name start cell");
    assert_eq!(nerd_name_start.symbol(), "a");
}

#[test]
fn an_out_of_window_cursor_paints_no_bar_and_does_not_panic() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    // The exact state `listnav::List`'s own `window_start_clamped_past_end`
    // test pins: `top` far past `len`, so `window()` returns an empty
    // range and the cursor sits outside it entirely.
    app.explorer.nav.cursor = 0;
    app.explorer.nav.top = 1000;

    let buf = testgrid::draw(&app, WIDTH, HEIGHT);
    let cursor_bg = app.theme.chrome.row_cursor_bg.bg.unwrap();
    let inner = explorer_inner(&app);
    for y in inner.y..inner.y + inner.height {
        for x in inner.x..inner.x + inner.width {
            let cell = buf.cell((x, y)).expect("in-bounds cell");
            assert_ne!(cell.bg, cursor_bg, "no row is in the empty window to bar");
        }
    }
}

#[test]
fn tabs_active_row_is_always_shown_and_the_cursor_row_only_when_tabs_is_focused_and_wins_on_overlap()
 {
    let mut app = App::new(Buffer::new("first"), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().viewport.set_size(80, 23);
    app.frame_width = WIDTH;
    app.frame_height = HEIGHT;
    app.splits.left.show();
    let active = app.active;
    let second = app.open_document(Buffer::new("second"));
    let _ = second;

    app.tabs.nav.cursor = 0; // sits on `active`'s own row
    app.tabs.nav.top = 0;

    let area = ratatui::layout::Rect::new(0, 0, WIDTH, HEIGHT);
    let geo = rune_tui::layout::geometry(area, &app);
    let tabs_inner = geo.tabs_inner;
    let active_idx = app
        .documents
        .order()
        .iter()
        .position(|&id| id == active)
        .expect("active doc is in documents.order()");
    let row_y = tabs_inner.y + active_idx as u16;

    let active_bg = app.theme.chrome.row_active_bg.bg.unwrap();
    let cursor_bg = app.theme.chrome.row_cursor_bg.bg.unwrap();

    // Unfocused: the active row still carries `row_active_bg`.
    let unfocused_buf = testgrid::draw(&app, WIDTH, HEIGHT);
    assert_eq!(
        unfocused_buf.cell((tabs_inner.x, row_y)).unwrap().bg,
        active_bg,
        "the active document's row is shown regardless of focus"
    );

    // Focused, cursor also on the active row: `row_cursor_bg` must win.
    let mut effects = Effects::default();
    app.set_focus_pane(Pane::Tabs, &mut effects);
    let focused_buf = testgrid::draw(&app, WIDTH, HEIGHT);
    assert_eq!(
        focused_buf.cell((tabs_inner.x, row_y)).unwrap().bg,
        cursor_bg,
        "the cursor bg must win where the active and cursor rows overlap"
    );
}
