//! TestBackend integration tests for the left column's chrome: ONE bordered
//! block holding both panes, its focus-colored border and in-block `Open`
//! divider, `^b` toggling the column end-to-end through the real
//! `app::update`, and the footer's `Ln n, Col n` on a multiline buffer with
//! a multibyte character (col in runes).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use ratatui::buffer::Buffer as RtBuffer;
use ratatui::style::Color;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;
/// The left column's default width, as `layout::DEFAULT_LEFT_PANE_W` sets
/// it for a terminal this wide.
const LEFT_W: u16 = 22;

fn app_for(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn draw(app: &App) -> RtBuffer {
    draw_with_width(app, WIDTH)
}

fn draw_with_width(app: &App, width: u16) -> RtBuffer {
    testgrid::draw(app, width, HEIGHT)
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

/// The `active_border`/`inactive_border` fg color of the cell at `(x, y)` —
/// `None` if that cell has no fg set at all (shouldn't happen on a border
/// cell, but a test failure here is more legible as a color mismatch than a
/// panic).
fn fg_at(buf: &RtBuffer, x: u16, y: u16) -> Option<Color> {
    buf.cell((x, y)).and_then(|c| c.style().fg)
}

/// The left column's own text, row by row, cut to the column's width.
fn left_column_rows(buf: &RtBuffer) -> Vec<String> {
    (0..HEIGHT - 1)
        .map(|y| row_text(buf, y, LEFT_W))
        .collect::<Vec<_>>()
}

/// The left column is ONE bordered block: `focus == Explorer` colors that
/// single border ACTIVE end to end — top edge AND the bottom edge on the
/// main area's last row (HEIGHT - 2; HEIGHT - 1 is the footer).
#[test]
fn left_pane_shows_one_bordered_block_with_a_focus_colored_border() {
    let mut app = app_for("hello");
    app.splits.left.show();
    app.set_focus_pane(Pane::Explorer, &mut Effects::default());
    app.sync_view();

    let buf = draw(&app);
    let active = app.theme.chrome.active_border.fg.unwrap();

    let top_border = fg_at(&buf, 1, 0).expect("top border cell has a style");
    assert_eq!(
        top_border, active,
        "focused Explorer's border must be the ACTIVE color"
    );

    let bottom_border = fg_at(&buf, 1, HEIGHT - 2).expect("bottom border cell has a style");
    assert_eq!(
        bottom_border, active,
        "the SAME block's bottom edge must carry the same color"
    );

    let top_row = row_text(&buf, 0, WIDTH);
    assert!(top_row.contains("Files"), "expected the Files pane title");
}

/// Exactly one top edge and one bottom edge for the whole column — the two
/// stacked blocks used to put a `╰…╯` / `╭…╮` pair in the middle of it.
#[test]
fn the_left_column_has_no_interior_border_rule() {
    let mut app = app_for("hello");
    app.splits.left.show();
    app.set_focus_pane(Pane::Explorer, &mut Effects::default());
    app.sync_view();

    let buf = draw(&app);
    let rows = left_column_rows(&buf);

    let tops = rows.iter().filter(|r| r.starts_with('\u{256d}')).count();
    let bottoms = rows.iter().filter(|r| r.starts_with('\u{2570}')).count();
    let joined = rows.join("\n");
    assert_eq!(tops, 1, "expected exactly ONE top border row in:\n{joined}");
    assert_eq!(
        bottoms, 1,
        "expected exactly ONE bottom border row in:\n{joined}"
    );
    assert!(
        rows.first().is_some_and(|r| r.starts_with('\u{256d}')),
        "the one top border row must be the column's first row:\n{joined}"
    );
    assert!(
        rows.last().is_some_and(|r| r.starts_with('\u{2570}')),
        "the one bottom border row must be the column's last row:\n{joined}"
    );
}

/// The Open Tabs section is introduced by an in-block divider row: side
/// border, ` Open `, then a `─` fill to the column's other side border.
#[test]
fn the_open_divider_row_renders_inside_the_single_border() {
    let mut app = app_for("hello");
    app.splits.left.show();
    app.set_focus_pane(Pane::Explorer, &mut Effects::default());
    app.sync_view();

    let buf = draw(&app);
    let rows = left_column_rows(&buf);
    let joined = rows.join("\n");
    let divider = rows
        .iter()
        .find(|r| r.contains("Open"))
        .unwrap_or_else(|| panic!("expected an Open divider row in:\n{joined}"));

    let expected = format!(
        "\u{2502} Open {}\u{2502}",
        "\u{2500}".repeat(LEFT_W as usize - 8)
    );
    assert_eq!(divider, &expected, "in:\n{joined}");
}

/// Tabs focus is signalled by the divider row's color alone — the block's
/// own border keeps tracking the Explorer.
#[test]
fn tabs_focus_colors_the_divider_not_the_border() {
    let mut app = app_for("hello");
    app.splits.left.show();
    app.set_focus_pane(Pane::Tabs, &mut Effects::default());
    app.sync_view();

    let buf = draw(&app);
    let rows = left_column_rows(&buf);
    let divider_y = rows
        .iter()
        .position(|r| r.contains("Open"))
        .map(|y| u16::try_from(y).unwrap_or(0))
        .expect("an Open divider row");

    assert_eq!(
        fg_at(&buf, 2, divider_y),
        app.theme.chrome.active_border.fg,
        "a focused Tabs pane must color its divider with the ACTIVE color"
    );
    assert_eq!(
        fg_at(&buf, 1, 0),
        app.theme.chrome.inactive_border.fg,
        "the border still tracks the Explorer, which is NOT focused here"
    );
}

/// Unfocused, the divider wears the subtle divider style instead.
#[test]
fn an_unfocused_tabs_pane_uses_the_subtle_divider_style() {
    let mut app = app_for("hello");
    app.splits.left.show();
    app.set_focus_pane(Pane::Explorer, &mut Effects::default());
    app.sync_view();

    let buf = draw(&app);
    let rows = left_column_rows(&buf);
    let divider_y = rows
        .iter()
        .position(|r| r.contains("Open"))
        .map(|y| u16::try_from(y).unwrap_or(0))
        .expect("an Open divider row");

    assert_eq!(
        fg_at(&buf, 2, divider_y),
        app.theme.chrome.tabs_divider.fg,
        "an unfocused Tabs pane's divider must use the subtle style"
    );
}

/// The editor pane's geometry is unchanged when the left column isn't
/// shown — no left column, no borders, full-width editor.
#[test]
fn left_pane_hidden_by_default_leaves_editor_geometry_unchanged() {
    let app = app_for("hello");
    assert!(!app.splits.left.is_shown());
    let buf = draw(&app);
    let top_row = row_text(&buf, 0, WIDTH);
    assert!(
        !top_row.contains("Files"),
        "no left pane chrome expected when the left column isn't shown:\n{top_row}"
    );
}

/// The two `App` constructors are the launch-mode seam for the left
/// column's initial visibility: `App::new_untitled` (no file argument) shows
/// it so the user has somewhere to navigate from, while `App::new` (a file
/// argument, exercised here through `app_for`) leaves it hidden so the
/// editor gets the full width for the document the user asked to open.
#[test]
fn an_untitled_app_starts_with_the_left_column_visible() {
    let app = App::new_untitled(Arc::new(Mem::new()), None);
    assert!(app.splits.left.is_shown());
}

#[test]
fn a_file_backed_app_starts_with_the_left_column_hidden() {
    let app = app_for("hello");
    assert!(!app.splits.left.is_shown());
}

/// `^b` end-to-end through the real `app::update`: shows the left column,
/// focuses the Explorer, and the very next render shows the bordered
/// blocks — one of `FocusExplorer`'s two direct chords (`crates/rune-tui/
/// tests/focus_chords.rs` covers both forms across every pane).
#[test]
fn ctrl_b_toggles_the_explorer_through_update() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('b'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );
    assert!(app.splits.left.is_shown());
    assert_eq!(app.focus(), Pane::Explorer);

    app.sync_view();
    let buf = draw(&app);
    assert!(row_text(&buf, 0, WIDTH).contains("Files"));
}

/// With the Explorer focused (the contextual hints
/// that can now grow past `GLOBAL_BINDINGS`' old fixed set), `Ln n, Col n`
/// must still be visible on the footer row at both a full-width (80) and a
/// narrow (40) terminal — the priority truncation reserves room for it
/// first, so it can never fall off regardless of how many hint entries fit.
#[test]
fn footer_position_readout_survives_truncation_at_narrow_widths() {
    for width in [80u16, 40u16] {
        let mut app = app_for("hello");
        app.splits.left.show();
        app.set_focus_pane(Pane::Explorer, &mut Effects::default());
        app.sync_view();

        let buf = draw_with_width(&app, width);
        let footer_row = row_text(&buf, HEIGHT - 1, width);
        assert!(
            footer_row.contains("Ln 1, Col 1"),
            "width {width}: expected 'Ln 1, Col 1' on the footer row:\n{footer_row}"
        );
    }
}

/// `Ln n, Col n` on a multiline buffer with a multibyte character before
/// the cursor: the column counts terminal CELLS, not bytes and not chars
/// — a CJK ideograph before the cursor counts as 2 columns, not 1.
#[test]
fn footer_reports_cell_column_on_a_multiline_multibyte_buffer() {
    let mut app = app_for("first\nab\u{6c49}cd");
    let id = app.active;
    // Byte offset right after "first\nab\u{6c49}" — 'a'(1) + 'b'(1) +
    // '\u{6c49}'(2 cells, CJK) precede it, so the display column is 4
    // cells, 1-indexed as 5.
    let offset = "first\nab\u{6c49}".len();
    app.doc_mut(id).unwrap().cursors = CursorSet::new(offset);
    app.sync_view();

    let buf = draw(&app);
    let footer_row = row_text(&buf, HEIGHT - 1, WIDTH);
    assert!(
        footer_row.contains("Ln 2, Col 5"),
        "expected 'Ln 2, Col 5' (cell column, not byte or char column) on the footer row:\n{footer_row}"
    );
}

/// `^d` is a pure alias of `^c` for quitting: the footer's default hints
/// must name the quit chord once, not twice, so the alias filter over
/// `GLOBAL_BINDINGS` is doing its job end to end.
#[test]
fn default_footer_hints_omit_the_aliased_quit_chord() {
    let app = app_for("hello");
    assert_eq!(app.focus(), Pane::Editor);
    let text = rune_tui::footer::footer_text(&app);
    assert!(
        text.contains("^C"),
        "expected the primary quit chord in {text:?}"
    );
    assert!(
        !text.contains("^D"),
        "the aliased quit chord must not appear in the footer: {text:?}"
    );
}

/// The always-available global tail (`F1 help`, `^C quit`) must survive
/// width truncation even once a focused pane's own hint table has grown the
/// row past what fits — a stable position ahead of the pane-specific table
/// is what keeps the row's most important entries from being the first
/// thing dropped under pressure. Renders through `draw`, the TRUNCATED path
/// (`footer_text` is untruncated and cannot observe this).
#[test]
fn footer_global_tail_survives_truncation_with_explorer_focused() {
    let mut app = app_for("hello");
    app.splits.left.show();
    app.set_focus_pane(Pane::Explorer, &mut Effects::default());
    app.sync_view();

    let buf = draw_with_width(&app, 120);
    let footer_row = row_text(&buf, HEIGHT - 1, 120);
    assert!(
        footer_row.contains("F1"),
        "expected 'F1' (help) on the truncated footer row:\n{footer_row}"
    );
    assert!(
        footer_row.contains("^C"),
        "expected '^C' (quit) on the truncated footer row:\n{footer_row}"
    );
}

/// `truncated_default_hint_spans` drops the first non-fitting hint AND
/// everything after it, so a hint inserted ahead of the table's tail
/// (`^N new`, mid-table) could in principle push a later hint off-screen
/// with nothing to catch it (the untruncated
/// `default_mode_lists_every_global_binding_label` test can't observe
/// truncation at all). Renders through `draw` at a realistic width and
/// asserts BOTH the new hint and the table's own tail (`^E messages`)
/// survive — measured, both fit within 120 columns; `trash` (the next
/// entry) is the actual cutoff, confirming truncation drops whole hints
/// from the low-priority end rather than the middle.
#[test]
fn footer_survives_truncation_with_new_document_hint_at_width_120() {
    let app = app_for("hello");
    assert_eq!(app.focus(), Pane::Editor);

    let buf = draw_with_width(&app, 120);
    let footer_row = row_text(&buf, HEIGHT - 1, 120);
    assert!(
        footer_row.contains("^N new"),
        "expected '^N new' on the width-120 footer row:\n{footer_row}"
    );
    assert!(
        footer_row.contains("^E messages"),
        "expected '^E messages' on the width-120 footer row:\n{footer_row}"
    );
    assert!(
        !footer_row.contains("trash"),
        "expected 'trash' to be the whole-hint cutoff at width 120, but it rendered:\n{footer_row}"
    );
}

/// The user-reported "blank last column" defect: the centre block's right
/// border must land on the FRAME's actual last column — `width - 1` — not
/// one short of it. Checked against the non-trimming `row_text` (unlike
/// `title_breadcrumb.rs`'s old `trim_end()`, which would have masked a
/// short row here) at both the top border row and an ordinary editor row,
/// across a couple of widths so the check isn't pinned to one accidental
/// size.
#[test]
fn the_center_blocks_right_border_reaches_the_last_frame_column() {
    for width in [WIDTH, 120] {
        let app = app_for("hello");
        let buf = draw_with_width(&app, width);

        let top_row = row_text(&buf, 0, width);
        assert_eq!(
            top_row.chars().last(),
            Some('\u{256e}'),
            "width {width}: expected the top-right border glyph on the LAST column of:\n{top_row}"
        );

        let editor_row = row_text(&buf, 1, width);
        assert_eq!(
            editor_row.chars().last(),
            Some('\u{2502}'),
            "width {width}: expected the right border glyph on the LAST column of:\n{editor_row}"
        );
    }
}

/// Aliases stay discoverable in the generated Help doc even though the
/// footer hides them — `^d` still works, so it must still be documented.
#[test]
fn help_markdown_still_lists_the_aliased_quit_chord() {
    let markdown = rune_tui::help::help_markdown();
    assert!(
        markdown.contains("^D"),
        "expected the aliased quit chord to remain documented in Help:\n{markdown}"
    );
}
