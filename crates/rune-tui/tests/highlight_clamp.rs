//! Clamp-plus-char-boundary-discard on the render path — a hostile
//! reply carrying an out-of-bounds, inverted, or off-char-boundary range
//! must be clamped or dropped before anything can paint it, never applied
//! verbatim.
//!
//! The clamp lives at the query, not at receipt: the render path has no
//! `&mut Document` and so cannot clamp on its own, and putting it there
//! covers both channels and both the fence and whole-file paths at once.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod highlight_common;

use highlight_common::{all_spans, app_for, span_reply};
use rune_syntax::scope::scope_table;
use rune_tui::app;
use rune_tui::highlight::PassOutcome;
use rune_tui::runtime::{Effects, Msg};

#[test]
// The `5..3` payload below is a deliberately inverted range — exactly the
// hostile reply the query must discard — not an accidental reversed-
// iteration mistake.
#[allow(clippy::reversed_empty_ranges)]
fn clamps_and_drops_out_of_bounds_and_off_char_boundary_ranges() {
    // 3 CJK codepoints (3 bytes each) + `\n` = 10 bytes; byte 1 sits inside
    // the first codepoint, never on a `char` boundary.
    let content = "日本語\n";
    let mut session = app_for(content, "/x/main.rs");
    let id = session.app().active;
    let len = session.app().doc(id).expect("doc").buffer.content().len();
    assert_eq!(len, 10);
    let version = session.app().doc(id).expect("doc").buffer.version();
    let keyword = scope_table().resolve("keyword").expect("known scope");

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version,
            result: PassOutcome::Replace(span_reply(vec![
                (0..1000, keyword), // past the end -> clamped to `len`
                (5..3, keyword),    // inverted -> dropped
                (1..2, keyword),    // mid-char -> dropped
                (0..3, keyword),    // valid, char-boundary aligned
            ])),
        },
        &mut effects,
    );

    let spans = all_spans(session.app());
    let content = session
        .app()
        .doc(id)
        .expect("doc")
        .buffer
        .content()
        .to_string();
    for (range, _) in &spans {
        assert!(range.start < range.end);
        assert!(range.end <= content.len());
        assert!(content.is_char_boundary(range.start));
        assert!(content.is_char_boundary(range.end));
    }
    assert!(spans.contains(&(0..len, keyword)));
    assert!(spans.contains(&(0..3, keyword)));
    assert_eq!(
        spans.len(),
        2,
        "the inverted and mid-char ranges must be dropped"
    );
}

/// The clamp survives the buffer shrinking under a stored span: a reply is
/// clamped when it is READ, so a later edit can never leave an unpaintable
/// range reachable.
#[test]
fn a_stored_span_is_re_clamped_after_the_buffer_shrinks() {
    let mut session = app_for("fn main() {}\n", "/x/main.rs");
    let id = session.app().active;
    let keyword = scope_table().resolve("keyword").expect("known scope");
    let version = session.app().doc(id).expect("doc").buffer.version();

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version,
            result: PassOutcome::Replace(span_reply(vec![(0..13, keyword)])),
        },
        &mut effects,
    );

    {
        let app = session.app_mut();
        let doc = app.doc_mut(id).expect("doc");
        doc.buffer = doc.buffer.delete(2, 13).expect("in-bounds delete");
    }

    let content = session
        .app()
        .doc(id)
        .expect("doc")
        .buffer
        .content()
        .to_string();
    for (range, _) in all_spans(session.app()) {
        assert!(
            range.end <= content.len(),
            "span {range:?} outlives the shrunken content of length {}",
            content.len()
        );
    }
}
