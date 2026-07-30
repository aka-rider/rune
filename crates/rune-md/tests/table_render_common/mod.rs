//! Shared fixtures for the `table_render_*` sibling test files (split from
//! one combined file, §1.6): a synced `(Buffer, DocMachine)` pair, the
//! line-joining/width-measuring helpers every layout's own test group
//! reuses, and the Wrapped/Pivoted fixture table. `#![allow(dead_code)]`
//! because each consumer binary only calls a subset of these — the rest
//! would otherwise trip `-D warnings`' dead-code lint in that particular
//! binary, exactly like the `conceal_common` sibling in this same
//! directory.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;
use rune_md::emit::emit;
use rune_syntax::wrap::WrapMap;
use rune_syntax::wrap::grapheme_width;
use unicode_segmentation::UnicodeSegmentation;

pub fn synced(content: &str, cursor_offset: usize, focused: bool) -> (Buffer, DocMachine) {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_focus(focused);
    doc.sync_content(&buf);
    let offset = cursor_offset.min(buf.len());
    let cursors = CursorSet::new(offset);
    doc.sync_cursors(&buf, &cursors);
    (buf, doc)
}

pub fn joined_line(lines: &[rune_syntax::SyntaxLine], line: usize, content: &str) -> String {
    lines
        .get(line)
        .map(|l| l.spans.iter().map(|s| s.text(content)).collect::<String>())
        .unwrap_or_default()
}

pub fn display_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

/// A row's rendered width, measured PER SPAN — each span's own text
/// grapheme-segmented independently, then summed — rather than joining
/// every span's text into one string first. This is the honest oracle
/// (WP9.S2): the renderer itself grapheme-segments each `SyntaxSpan`'s text
/// independently (never across a span boundary), so joining spans back into
/// one string before measuring can silently re-fuse a grapheme cluster the
/// render already tore apart at a span boundary, making the oracle agree
/// with a wrong `col_widths` measurement instead of catching the disagreement.
pub fn per_span_display_width(lines: &[rune_syntax::SyntaxLine], line: usize, content: &str) -> usize {
    lines
        .get(line)
        .map(|l| l.spans.iter().map(|s| display_width(s.text(content))).sum())
        .unwrap_or(0)
}

/// A 65-char URL, a wide-but-word-short "Description" column, and a short
/// "Name" column — sized (worked out against `layout::choose`'s own
/// formulas) so the table's natural Grid width does not fit at 100 columns
/// but Wrapped viably does, and so it collapses all the way to Pivoted at
/// 20 columns. One row only, so `include_separator` is `false` and the
/// Pivoted case has exactly one record to check.
pub fn wrap_pivot_url() -> String {
    let url: String = format!("https://{}", "a".repeat(57));
    assert_eq!(url.chars().count(), 65, "fixture must stay a 65-char URL");
    format!(
        "| Name | Description | URL |\n| --- | --- | --- |\n| Alice | quick brown fox jumps over lazy dog | {url} |\n"
    )
}

pub fn display_rows_at(buf: &Buffer, doc: &DocMachine, width: u16) -> Vec<String> {
    let (lines, _snap) = emit(buf.content(), doc.blocks(), width);
    let wrap = WrapMap::new(width).sync(buf.content(), &lines);
    let display = rune_md::snapshot::DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
    display
        .rows()
        .iter()
        .map(|r| {
            r.spans
                .iter()
                .map(|s| s.text(buf.content()))
                .collect::<String>()
        })
        .collect()
}
