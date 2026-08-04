//! Characterizes a setext-heading conceal defect at the rendered screen
//! level (repro only — no production fix here). CommonMark makes a lone
//! `-`/`===` line right after paragraph text a setext heading underline,
//! so comrak reports the whole thing as `Block::Heading`. Rune conceals a
//! heading by hiding `h.marker`, but `HeadingM::marker` is an EMPTY range
//! for a setext heading (there is no leading `"## "`-style prefix to
//! hide) — so the underline row is never hidden, never decorated, and
//! renders as plain unstyled text sitting under a concealed, icon-
//! decorated heading. An ATX heading (document 3) has a real, non-empty
//! marker and is the control: it must render correctly throughout.
//!
//! Follows `heading_style.rs`'s local-helper idiom rather than
//! `tui_render_common`: `find_col`'s cell-by-cell scan is needed here too,
//! for the same reason that file states — multi-byte border glyphs make
//! byte offsets disagree with backend columns.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::buffer::Buffer as RtBuffer;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::icons::IconSet;
use rune_syntax::scope::scope_table;
use rune_tui::app::App;
use rune_tui::testgrid;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// First editor content row: the rows above it are pane chrome (title and
/// breadcrumb), the same accounting the other render tests pin.
const EDITOR_TOP_ROW: u16 = 2;

/// `App::new` already sets `App::icons` to `IconSet::unicode()` directly
/// (never through `theme::icons::choose`, which only ever runs once at
/// real startup, reading `RUNE_ICONS`/`TERM_PROGRAM`/`TERM`) — so a test
/// fixture that never touches those env vars or `app.icons` itself is
/// already pinned to the unicode tier regardless of the host terminal.
/// This assertion makes that pin explicit rather than relying on it
/// silently: if `App::new`'s default ever changes, this test — not a
/// flaky icon-glyph mismatch three assertions later — is what fails.
fn app_for(content: &str, cursor_offset: usize) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    assert_eq!(
        app.icons,
        IconSet::unicode(),
        "fixture relies on App::new's own unicode default, not env-driven selection"
    );
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset.min(content.len()));
    app.doc_mut(id)
        .unwrap()
        .viewport
        .set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn render(app: &App) -> RtBuffer {
    testgrid::draw(app, WIDTH, HEIGHT)
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

/// The backend COLUMN (not a byte offset into the joined row string —
/// `row_text`'s multi-byte border/icon glyphs make those two disagree) at
/// which `needle`'s cell-by-cell symbol sequence starts on row `y`,
/// scanning cell by cell rather than through a concatenated string.
fn find_col(buf: &RtBuffer, y: u16, width: u16, needle: &str) -> Option<u16> {
    let want: Vec<&str> = needle.split("").filter(|s| !s.is_empty()).collect();
    (0..width).find(|&x| {
        want.iter().enumerate().all(|(i, sym)| {
            buf.cell((x + i as u16, y))
                .is_some_and(|cell| cell.symbol() == *sym)
        })
    })
}

fn dump(label: &str, buf: &RtBuffer, height: u16, width: u16) {
    println!("--- {label} ---");
    for y in 0..height {
        println!("{y:>2} |{}|", row_text(buf, y, width));
    }
}

fn heading_style(app: &App, level: u8) -> ratatui::style::Style {
    app.theme.scope_style(
        scope_table()
            .resolve(&format!("markup.heading.{level}"))
            .expect("known scope"),
    )
}

fn cell_carries_style(buf: &RtBuffer, x: u16, y: u16, expected: ratatui::style::Style) -> bool {
    buf.cell((x, y)).is_some_and(|cell| {
        cell.style().fg == expected.fg && cell.modifier == expected.add_modifier
    })
}

/// Document 1: a bare setext H2 ("Title" underlined by a lone `---`).
/// Cursor parked on the unrelated "body" line, so the heading's own
/// `RevealSm` (decided solely off `cursors.any_on_line(h.line)`, i.e. the
/// "Title" line — never the underline's line) stays Rendered/concealed.
#[test]
fn concealed_setext_heading_leaves_its_underline_row_on_screen() {
    let content = "Title\n---\nbody\n";
    let cursor = content.find("body").expect("fixture has a body line");
    let app = app_for(content, cursor);
    let buf = render(&app);
    dump("doc1 cursor off heading (concealed)", &buf, HEIGHT, WIDTH);

    let title_row = row_text(&buf, EDITOR_TOP_ROW, WIDTH);
    let underline_row = row_text(&buf, EDITOR_TOP_ROW + 1, WIDTH);
    let body_row = row_text(&buf, EDITOR_TOP_ROW + 2, WIDTH);

    assert!(
        title_row.contains("Title"),
        "heading text must render:\n{title_row}"
    );
    assert!(
        underline_row.trim_end().ends_with("---") || underline_row.contains("---"),
        "the setext underline is not hidden and stays on screen as bare prose:\n{underline_row}"
    );
    assert!(
        body_row.contains("body"),
        "body text renders on its own row:\n{body_row}"
    );

    let title_col = find_col(&buf, EDITOR_TOP_ROW, WIDTH, "Title").expect("heading text renders");
    let expected = heading_style(&app, 2);
    assert!(
        cell_carries_style(&buf, title_col, EDITOR_TOP_ROW, expected),
        "the concealed heading's own text must carry markup.heading.2 (the real control: without this, the underline test below would be meaningless)"
    );

    let dash_col = find_col(&buf, EDITOR_TOP_ROW + 1, WIDTH, "-").expect("dash renders");
    assert!(
        !cell_carries_style(&buf, dash_col, EDITOR_TOP_ROW + 1, expected),
        "the underline row is unstyled prose, not concealed and not decorated like the heading it belongs to"
    );
}

/// Same document, cursor moved onto the underline row itself. `HeadingM::
/// sync` decides Reveal/Conceal off `cursors.any_on_line(h.line)` alone —
/// `h.line` is the heading's TEXT line, not the underline's — so parking
/// the caret on the bare `---` does not reveal the heading: the heading
/// stays concealed while the cursor itself sits on an unconcealed,
/// unstyled row that is nominally part of that same concealed heading.
#[test]
fn cursor_on_setext_underline_does_not_reveal_the_heading() {
    let content = "Title\n---\nbody\n";
    let cursor = content.find("---").expect("fixture has an underline");
    let app = app_for(content, cursor);
    let buf = render(&app);
    dump("doc1 cursor on underline row", &buf, HEIGHT, WIDTH);

    let title_row = row_text(&buf, EDITOR_TOP_ROW, WIDTH);
    assert!(
        title_row.contains("Title") && !title_row.contains("---"),
        "the heading text line shows no raw markup, i.e. it is still concealed even with the caret on the underline:\n{title_row}"
    );

    let title_col = find_col(&buf, EDITOR_TOP_ROW, WIDTH, "Title").expect("heading text renders");
    let expected = heading_style(&app, 2);
    assert!(
        cell_carries_style(&buf, title_col, EDITOR_TOP_ROW, expected),
        "heading text still carries markup.heading.2 while concealed"
    );

    let underline_row = row_text(&buf, EDITOR_TOP_ROW + 1, WIDTH);
    assert!(
        underline_row.contains("-"),
        "the underline is still on screen, now directly under the caret:\n{underline_row}"
    );
}

/// Document 2, the user's real shape: typing `- ` under an existing list
/// item flips the previous item's paragraph ("**a**: b") into a setext H2,
/// with the new lone `-` line becoming its unconcealed, unstyled
/// underline row.
#[test]
fn concealed_setext_heading_inside_a_list_item_leaves_its_underline_on_screen() {
    let content = "- **a**: b\n  -\n- next\n";
    let cursor = content.rfind("next").expect("fixture has a next item");
    let app = app_for(content, cursor);
    let buf = render(&app);
    dump(
        "doc2 (list-item setext) cursor off heading",
        &buf,
        HEIGHT,
        WIDTH,
    );

    let heading_row = row_text(&buf, EDITOR_TOP_ROW, WIDTH);
    let underline_row = row_text(&buf, EDITOR_TOP_ROW + 1, WIDTH);
    let next_row = row_text(&buf, EDITOR_TOP_ROW + 2, WIDTH);

    assert!(
        heading_row.contains("a") && heading_row.contains('b') && !heading_row.contains("**"),
        "the flattened emphasis heading text renders with its markers concealed:\n{heading_row}"
    );
    assert!(
        underline_row.trim().contains('-'),
        "the bare dash stays on screen instead of being hidden as the heading's marker:\n{underline_row}"
    );
    assert!(
        next_row.contains("next"),
        "the next list item renders on its own row:\n{next_row}"
    );

    let a_col = find_col(&buf, EDITOR_TOP_ROW, WIDTH, "a").expect("heading text renders");
    let expected = heading_style(&app, 2);
    assert!(
        cell_carries_style(&buf, a_col, EDITOR_TOP_ROW, expected),
        "the previous list item's text now carries markup.heading.2 — it flipped to a heading"
    );
}

/// Control: an ATX heading has a real, non-empty `marker` ("## "), so
/// `hide_range(h.marker)` actually hides something and no bare markup
/// leaks onto the screen. If this test failed, the two tests above would
/// not be characterizing a setext-specific defect.
#[test]
fn atx_heading_conceals_correctly() {
    let content = "## Title\nbody\n";
    let cursor = content.find("body").expect("fixture has a body line");
    let app = app_for(content, cursor);
    let buf = render(&app);
    dump("doc3 (ATX control)", &buf, HEIGHT, WIDTH);

    let title_row = row_text(&buf, EDITOR_TOP_ROW, WIDTH);
    assert!(
        !title_row.contains("## "),
        "the ATX marker must be concealed:\n{title_row}"
    );
    assert!(
        title_row.contains("Title"),
        "heading text still renders:\n{title_row}"
    );

    let title_col = find_col(&buf, EDITOR_TOP_ROW, WIDTH, "Title").expect("heading text renders");
    let expected = heading_style(&app, 2);
    assert!(
        cell_carries_style(&buf, title_col, EDITOR_TOP_ROW, expected),
        "ATX heading text carries markup.heading.2"
    );
}

/// Target behaviour once the fix lands: the setext underline row should
/// be hidden while concealed, the same as an ATX marker — no bare `---`
/// left over on its own row. Unignore this alongside that fix.
#[test]
#[ignore = "documents the target behavior; unignore with the fix"]
fn concealed_setext_heading_should_hide_its_underline_row() {
    let content = "Title\n---\nbody\n";
    let cursor = content.find("body").expect("fixture has a body line");
    let app = app_for(content, cursor);
    let buf = render(&app);

    let underline_row = row_text(&buf, EDITOR_TOP_ROW + 1, WIDTH);
    let body_row = row_text(&buf, EDITOR_TOP_ROW + 2, WIDTH);
    assert!(
        !underline_row.contains("---"),
        "the setext underline must be hidden while the heading is concealed:\n{underline_row}"
    );
    assert!(
        body_row.contains("body"),
        "body must shift up onto the underline's freed row:\n{body_row}"
    );
}
