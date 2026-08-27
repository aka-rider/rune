//! WP4.S3: `DocMachine::set_kind(DocumentKind::Plain)` makes `sync_content`
//! skip the comrak parse entirely (`blocks` stays empty); `snapshot` is left
//! untouched and its `emit` call turns that empty block list into one
//! verbatim `Identical` span per line via the existing `fill_gaps` pass —
//! there is no second plain-text producer (plan decision 6).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;
use rune_syntax::DocumentKind;
use rune_syntax::SyntaxSpan;

#[test]
fn plain_kind_skips_parse_and_emits_source_bytes_verbatim() {
    let content = "# not a heading\n\tliteral tab\n";
    let buf = Buffer::new(content);

    let mut doc = DocMachine::new();
    doc.set_kind(DocumentKind::Plain);
    doc.sync_content(&buf);
    assert!(
        doc.blocks().is_empty(),
        "a Plain document must never run the comrak parse"
    );

    doc.set_reveal_mode(true.into());
    doc.set_width(80);
    doc.sync_cursors(&buf, &CursorSet::new(0), &[]);
    let view = doc.snapshot(&buf);

    // Width 80 is wider than either line, so no wrap splitting occurs: one
    // display row per buffer line (the trailing empty line from the final
    // `\n` included, carrying no span at all).
    let expected_lines: Vec<&str> = content.split('\n').collect();
    let rows = view.display.rows();
    assert_eq!(rows.len(), expected_lines.len());

    for (row, expected) in rows.iter().zip(expected_lines.iter()) {
        for span in &row.spans {
            assert!(
                !matches!(span, SyntaxSpan::Substituted { .. }),
                "a Plain document must never conceal or substitute bytes: {span:?}"
            );
        }
        let reconstructed: String = row.spans.iter().map(|s| s.text(content)).collect();
        assert_eq!(&reconstructed, expected);
    }
}
