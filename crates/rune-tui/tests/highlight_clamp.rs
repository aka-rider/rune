//! Split off `highlight.rs` (WP11, §1.6): clamp-plus-char-boundary-discard
//! on receipt (§1.3) — a hostile `Msg::Highlighted` reply carrying an
//! out-of-bounds, inverted, or off-char-boundary range must be clamped or
//! dropped, never applied verbatim.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod highlight_common;

use highlight_common::app_for;
use rune_syntax::scope::scope_table;
use rune_tui::app;
use rune_tui::runtime::{Effects, HighlightPayload, Msg};

#[test]
// The `5..3` payload below is a deliberately inverted range — exactly the
// hostile reply `handle_highlighted` must discard — not an accidental
// reversed-iteration mistake.
#[allow(clippy::reversed_empty_ranges)]
fn clamps_and_drops_out_of_bounds_and_off_char_boundary_ranges() {
    // 3 CJK codepoints (3 bytes each) + `\n` = 10 bytes; byte 1 sits inside
    // the first codepoint, never on a `char` boundary.
    let content = "日本語\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    let len = app.doc(id).expect("doc").buffer.content().len();
    assert_eq!(len, 10);
    let version = app.doc(id).expect("doc").buffer.version();
    let keyword = scope_table().resolve("keyword").expect("known scope");

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: Some(HighlightPayload::Spans(
                vec![
                    (0..1000, keyword), // past the end -> clamped to `len`
                    (5..3, keyword),    // inverted -> dropped
                    (1..2, keyword),    // mid-char -> dropped
                    (0..3, keyword),    // valid, char-boundary aligned
                ]
                .into(),
            )),
        },
        &mut effects,
    );

    let doc = app.doc(id).expect("doc");
    let content = doc.buffer.content();
    for (range, _) in &doc.highlight.spans {
        assert!(range.start < range.end);
        assert!(range.end <= content.len());
        assert!(content.is_char_boundary(range.start));
        assert!(content.is_char_boundary(range.end));
    }
    assert!(doc.highlight.spans.contains(&(0..len, keyword)));
    assert!(doc.highlight.spans.contains(&(0..3, keyword)));
    assert_eq!(
        doc.highlight.spans.len(),
        2,
        "the inverted and mid-char ranges must be dropped"
    );
}
