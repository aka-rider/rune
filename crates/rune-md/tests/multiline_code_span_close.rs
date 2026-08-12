#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::needless_range_loop
)]

mod conceal_common;

use conceal_common::{joined_line, synced_at};
use rune_md::element::block::Block;
use rune_md::element::inline::Inline;
use rune_md::emit::emit;
use rune_syntax::SyntaxSpan;

#[test]
fn multiline_code_span_reveals_content_hides_delimiter() {
    let content = "a\n`\n e`";
    let (buf, doc) = synced_at(content, &[0], false, 78);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 78);
    assert_eq!(joined_line(&lines, 2, content), " e");
}

#[test]
fn multiline_fenced_open_code_span_reveals_content_hides_delimiter() {
    let content = "h\nô```\n  é```";
    let (buf, doc) = synced_at(content, &[0], false, 78);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 78);
    assert_eq!(joined_line(&lines, 2, content), "  é");
}

fn paragraph_inlines(content: &str) -> Vec<Inline> {
    let blocks = rune_md::parse::parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph, got {:?}", blocks[0]);
    };
    p.inlines.clone()
}

#[test]
fn unterminated_code_span_degrades_to_text() {
    let content = "a\n`\n e";
    let inlines = paragraph_inlines(content);
    assert!(
        inlines.iter().all(|i| !matches!(i, Inline::Code(_))),
        "an opening backtick with no matching close run must refuse InlineCodeM and stay text, got {inlines:?}"
    );
}

#[test]
fn unterminated_code_span_renders_every_byte() {
    for &focused in &[true, false] {
        let content = "a\n`\n e";
        let (buf, doc) = synced_at(content, &[0], focused, 78);
        let (lines, snap) = emit(buf.content(), doc.blocks(), 78);
        for line in 0..buf.line_count() {
            let joined = joined_line(&lines, line, content);
            assert_eq!(
                joined,
                buf.line(line),
                "line {line} (focused={focused}): unterminated span must render byte-identical, hiding nothing"
            );
            assert_eq!(snap.hidden_byte_count(line), 0);
        }
    }
}

#[test]
fn unterminated_code_span_fill_gaps_marks_identical() {
    let content = "a\n`\n e";
    let (buf, doc) = synced_at(content, &[0], false, 78);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 78);
    for line in 0..buf.line_count() {
        for span in &lines[line].spans {
            assert!(
                matches!(span, SyntaxSpan::Identical { .. }),
                "line {line}: refused span's bytes must surface through fill_gaps as Identical, got {span:?}"
            );
        }
    }
}
