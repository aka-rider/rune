//! Regression for the `TABLE-ROW-WIDTH` fuzz catch (`make test-fuzz`,
//! `crates/rune-fuzz/proptest-regressions/human_session.txt`,
//! `crates/rune-fuzz/artifacts/table-row-width-0ab10b82/`): a lone `\r`
//! (a CR NOT followed by `\n`) sitting before a GFM table used to desync
//! comrak's own CommonMark line count from this crate's `\n`-only buffer
//! line count. A sibling `Paragraph` and the table's own header row could
//! then land on the SAME buffer line, so `emit_table` (which keys every
//! row off a BUFFER line via `line_at`) rendered one display row carrying
//! BOTH the paragraph's text and a header row's box, wider than the
//! border synthesised from that row's own `col_widths` — the exact
//! `TABLE-ROW-WIDTH` violation. `parse::parse_shadow` fixes this at the
//! root: comrak now parses a copy with every lone `\r`
//! blanked to a space, so its own line count can never again disagree
//! with `starts` (`parse::line_starts`) — the two indexes this crate used
//! to carry (`LineIndex::{buffer,comrak}`) collapse into one by
//! construction, and the co-tenancy this test guards against becomes
//! unrepresentable rather than merely unasserted.
//!
//! A lone `\r` never counts as a line terminator: `"x\r| Name | Age |\n..."`
//! is ONE paragraph, not a paragraph followed by a table — this crate's
//! fix produces that same shape.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;
use rune_md::emit::emit;
use rune_syntax::SyntaxLine;
use rune_syntax::wrap::grapheme_width;
use unicode_segmentation::UnicodeSegmentation;

fn synced(content: &str, focused: bool) -> (Buffer, DocMachine) {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_reveal_mode(focused.into());
    doc.sync_content(&buf);
    doc.sync_cursors(&buf, &CursorSet::new(0));
    (buf, doc)
}

fn display_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

fn row_width(line: &SyntaxLine, content: &str) -> usize {
    display_width(
        &line
            .spans
            .iter()
            .map(|s| s.text(content))
            .collect::<String>(),
    )
}

/// Every contiguous run of lines carrying `Some(TableRowInfo)` is one
/// table group (`RowMeta::table_group` in the fuzzer's own invariant,
/// `crates/rune-fuzz/src/invariant/render.rs`'s `TABLE-ROW-WIDTH`): every
/// row inside it must share the SAME rendered display width, or the box
/// this table drew around itself is a lie about at least one of its own
/// rows.
fn assert_every_table_group_has_uniform_width(lines: &[SyntaxLine], content: &str) {
    let mut i = 0;
    let mut group = 0;
    while i < lines.len() {
        if lines[i].table.is_none() {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && lines[i].table.is_some() {
            i += 1;
        }
        let widths: Vec<usize> = lines[start..i]
            .iter()
            .map(|l| row_width(l, content))
            .collect();
        let first = widths.first().copied().unwrap_or(0);
        assert!(
            widths.iter().all(|&w| w == first),
            "table_group {group}: rows have mismatched summed widths {widths:?} \
             (lines {start}..{i})"
        );
        group += 1;
    }
}

/// The minimal repro from the `TABLE-ROW-WIDTH` fuzz catch: a lone `\r`
/// immediately before what would otherwise be a table's header row. Post-
/// fix, comrak no longer sees a line break there at all (see this module's
/// docs), so no `Table` block forms and there is no table group to check —
/// the assertion holds vacuously, which is exactly the point: the
/// co-tenancy that used to violate it can no longer arise.
#[test]
fn lone_cr_before_table_header_never_produces_a_mismatched_row_width() {
    let content = "x\r| Name | Age |\n| --- | --- |\n| Alice | 30 |\n\ntail\n";
    let (buf, doc) = synced(content, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_every_table_group_has_uniform_width(&lines, buf.content());
}

/// Same shape, closer to the fuzzer's own multi-row fixture
/// (`crates/rune-fuzz/artifacts/table-row-width-0ab10b82/report.txt`):
/// a lone `\r` before a two-body-row table.
#[test]
fn lone_cr_before_multi_row_table_never_produces_a_mismatched_row_width() {
    let content = "x\r| Name | Age |\n| :--- | ---: |\n| Alice | 30 |\n| Bob | 25 |\n\ntail\n";
    let (buf, doc) = synced(content, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_every_table_group_has_uniform_width(&lines, buf.content());
}

/// A genuine, still-recognized table (no lone `\r` anywhere near it) must
/// keep passing the same check — the helper above isn't trivially
/// vacuous, and an ordinary table's rows really are all the same width.
#[test]
fn ordinary_table_without_any_cr_has_uniform_row_width() {
    let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n";
    let (buf, doc) = synced(content, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_every_table_group_has_uniform_width(&lines, buf.content());
}
