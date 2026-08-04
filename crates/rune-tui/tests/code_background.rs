//! The code-region background is a RECTANGLE, not a text-shaped tint.
//!
//! A background carried on a `SyntaxSpan` could only colour cells that
//! existed, so it stopped at each line's last character, was entirely absent
//! on a blank line inside a code block, and never appeared behind a whole
//! code document at all. These tests state the replacement as a property:
//! every row of a code region is filled from the end of its own decoration
//! to the pane edge, and identical code paints an identical background
//! whether it sits in a fence or in a source file.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod highlight_common;

use highlight_common::app_for;
use ratatui::buffer::Buffer as RtBuffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::App;
use rune_tui::render;
use rune_tui::testgrid;

const W: u16 = 30;
const H: u16 = 14;

fn draw(app: &App) -> RtBuffer {
    testgrid::draw(app, W, H)
}

fn sized_app(content: &str, path: &str) -> App {
    let mut app = app_for(content, path);
    app.doc_mut(app.active)
        .expect("doc")
        .viewport
        .set_size(W, H);
    app.sync_view();
    app
}

/// The columns of screen row `y` whose background is the theme's code
/// background, as a `(first, last)` inclusive run — `None` when nothing on
/// that row is painted. Panics if the painted columns are not contiguous:
/// "a rectangle" is precisely the claim that they are.
fn painted_run(buf: &RtBuffer, app: &App, y: u16) -> Option<(u16, u16)> {
    let bg = app.theme.chrome.code_bg;
    let cols: Vec<u16> = (0..W)
        .filter(|&x| buf.cell((x, y)).is_some_and(|c| c.style().bg == Some(bg)))
        .collect();
    let first = *cols.first()?;
    let last = *cols.last()?;
    assert_eq!(
        cols.len() as u16,
        last - first + 1,
        "row {y}'s painted columns are not one contiguous run: {cols:?}"
    );
    Some((first, last))
}

fn row_text(buf: &RtBuffer, y: u16) -> String {
    (0..W)
        .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
        .collect()
}

/// The first screen row whose text contains `needle`.
fn row_containing(buf: &RtBuffer, needle: &str) -> u16 {
    (0..H)
        .find(|&y| row_text(buf, y).contains(needle))
        .unwrap_or_else(|| panic!("no rendered row contains {needle:?}"))
}

const FENCED: &str = concat!(
    "Intro.\n",
    "\n",
    "```rust\n",
    "fn main() {}\n",
    "\n",
    "let x = 1;\n",
    "```\n",
    "\n",
    "Outro.\n",
);

/// The whole point of the pass: the fill reaches the blank line in the
/// middle of the block and the ragged space past a short line, and every
/// row of the block gets the SAME run of columns. The prose rows around it
/// stay unpainted, so the rectangle has a real edge rather than bleeding
/// into the document.
#[test]
fn a_code_block_paints_a_rectangle_over_blank_and_short_lines_alike() {
    let app = sized_app(FENCED, "/x/notes.md");
    let buf = draw(&app);

    let long = row_containing(&buf, "fn main() {}");
    let blank = long + 1;
    let short = long + 2;
    assert!(
        !row_text(&buf, blank).chars().any(char::is_alphanumeric),
        "the row between the two code lines must be the block's blank line"
    );
    assert!(row_text(&buf, short).contains("let x = 1;"));

    let long_run = painted_run(&buf, &app, long).expect("the code row must be painted");
    let blank_run =
        painted_run(&buf, &app, blank).expect("a blank line inside a code block must be painted");
    let short_run = painted_run(&buf, &app, short).expect("the short code row must be painted");
    assert_eq!(
        blank_run, long_run,
        "a blank line must span exactly the same columns as the code around it"
    );
    assert_eq!(
        short_run, long_run,
        "a short line must span exactly the same columns as the code around it"
    );

    let (first, last) = long_run;
    assert!(
        usize::from(last - first + 1) > "fn main() {}".len(),
        "the fill must run past the text's own ragged right edge"
    );

    assert_eq!(
        painted_run(&buf, &app, row_containing(&buf, "Intro.")),
        None,
        "prose above the block must stay unpainted"
    );
    assert_eq!(
        painted_run(&buf, &app, row_containing(&buf, "Outro.")),
        None,
        "prose below the block must stay unpainted"
    );
}

/// The same code, once inside a fence and once as a whole source file,
/// paints the same background geometry — which is the reason region
/// membership is decided by `CodeRegion` rather than by a scope that only a
/// fence's own text happens to carry.
#[test]
fn a_fence_and_a_code_document_paint_the_same_background_geometry() {
    let fence_app = sized_app(FENCED, "/x/notes.md");
    let fence_buf = draw(&fence_app);
    let file_app = sized_app("fn main() {}\n\nlet x = 1;\n", "/x/main.ts");
    let file_buf = draw(&file_app);

    let fence_top = row_containing(&fence_buf, "fn main() {}");
    let file_top = row_containing(&file_buf, "fn main() {}");
    for delta in 0..3u16 {
        assert_eq!(
            painted_run(&fence_buf, &fence_app, fence_top + delta),
            painted_run(&file_buf, &file_app, file_top + delta),
            "code line {delta} paints a different background in a fence than in a source file"
        );
    }
}

/// A fence inside a blockquote paints AFTER the quote bar, never under it:
/// the bar keeps its own style and the rectangle starts at the first
/// content column.
#[test]
fn a_blockquoted_fence_paints_after_the_quote_bar_never_under_it() {
    let content = concat!(
        "Intro.\n",
        "\n",
        "> ```rust\n",
        "> fn main() {}\n",
        ">\n",
        "> let x = 1;\n",
        "> ```\n",
        "\n",
        "Outro.\n",
    );
    let app = sized_app(content, "/x/notes.md");
    let buf = draw(&app);

    let plain = sized_app(FENCED, "/x/notes.md");
    let plain_buf = draw(&plain);
    let plain_start = painted_run(
        &plain_buf,
        &plain,
        row_containing(&plain_buf, "fn main() {}"),
    )
    .expect("the unquoted fence must be painted")
    .0;

    let code = row_containing(&buf, "fn main() {}");
    let (first, _) = painted_run(&buf, &app, code).expect("the quoted fence must be painted");
    assert!(
        first > plain_start,
        "the background must start further right than an unquoted fence's, past the quote bar"
    );

    let bar = buf
        .cell((first - 1, code))
        .expect("a cell left of the fill");
    assert_ne!(
        bar.symbol(),
        " ",
        "the column left of the fill must be the quote bar's own glyph"
    );
    assert_ne!(
        bar.style().bg,
        Some(app.theme.chrome.code_bg),
        "the quote bar must keep its own style, never the code background"
    );
}

/// A Revealed fence shows its own ``` delimiter lines; those rows belong to
/// the region (`CodeRegion::rows` includes them) and are painted like any
/// other. A Rendered fence hides them — they render as blank rows, covered
/// by the same rule with no special case either way.
#[test]
fn a_revealed_fences_delimiter_rows_are_covered() {
    let mut app = sized_app(FENCED, "/x/notes.md");
    let inside = FENCED
        .find("fn main")
        .expect("fixture contains the code line");
    app.doc_mut(app.active).expect("doc").cursors = CursorSet::new(inside);
    app.sync_view();
    let buf = draw(&app);

    let opening = row_containing(&buf, "```rust");
    let code = row_containing(&buf, "fn main() {}");
    assert_eq!(
        opening + 1,
        code,
        "the revealed opening delimiter must sit directly above the code"
    );
    assert_eq!(
        painted_run(&buf, &app, opening),
        painted_run(&buf, &app, code),
        "a revealed delimiter row must be covered exactly like the body"
    );
}

/// The `SYNC-IDEMPOTENT` shape: the fill reads only the display snapshot,
/// the regions and the pane width, so re-syncing and re-rendering with no
/// message in between must reproduce the rows cell for cell — padding
/// included.
#[test]
fn two_message_free_renders_produce_identical_rows() {
    let mut app = sized_app(FENCED, "/x/notes.md");
    let before = {
        let view = app.active_doc().view.as_ref().expect("synced view");
        render::build_rows(view, &app)
    };
    app.sync_view();
    let after = {
        let view = app.active_doc().view.as_ref().expect("synced view");
        render::build_rows(view, &app)
    };
    assert_eq!(
        before, after,
        "a second sync_view + render with no message changed the rendered rows"
    );
}
