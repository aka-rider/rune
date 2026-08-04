//! WP2.S7: decor-producer tests, kept apart from the main emit tests so
//! both files stay under CONSTITUTION §1.6's limit. Three groups: (a) decor never
//! perturbs a line's own span bytes (the byte-neutrality Gotcha every decor
//! producer must respect); (b) decor is present iff the block is Rendered;
//! (c) a task item keeps its `☐`/`☑` checkbox substitution and gets NO
//! bullet decor on top of it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::emit_with;
use super::tests::synced;
use crate::icons::IconSet;
use rune_syntax::SyntaxSpan;

fn lines_for(content: &str, cursor_offset: usize, focused: bool) -> Vec<rune_syntax::SyntaxLine> {
    let (buf, doc) = synced(content, cursor_offset, focused);
    let (lines, _snap) = emit_with(
        buf.content(),
        doc.blocks(),
        80,
        &IconSet::unicode(),
        super::style::text_scope(),
    );
    lines
}

fn joined_text(line: &rune_syntax::SyntaxLine, content: &str) -> String {
    line.spans.iter().map(|s| s.text(content)).collect()
}

// --- (a) decor never changes a line's own span bytes -----------------

#[test]
fn bullet_decor_does_not_perturb_the_lines_span_text() {
    let content = "- item\n";
    let lines = lines_for(content, content.len(), false); // unfocused: Rendered
    assert_eq!(joined_text(&lines[0], content), "item");
    assert!(
        lines[0].decor.is_some(),
        "a Rendered bullet line must carry decor"
    );
}

#[test]
fn ordered_decor_does_not_perturb_the_lines_span_text() {
    let content = "1. item\n";
    let lines = lines_for(content, content.len(), false);
    assert_eq!(joined_text(&lines[0], content), "item");
    let decor = lines[0]
        .decor
        .as_ref()
        .expect("ordered item must carry decor");
    assert_eq!(decor.pieces.len(), 1);
    assert_eq!(decor.pieces[0].first, "1. ");
}

#[test]
fn quote_bar_decor_does_not_perturb_the_lines_span_text() {
    let content = "> q\n";
    let lines = lines_for(content, content.len(), false);
    assert_eq!(joined_text(&lines[0], content), "q");
    assert!(
        lines[0].decor.is_some(),
        "a Rendered quote line must carry decor"
    );
}

#[test]
fn hr_decor_does_not_perturb_the_lines_span_text() {
    let content = "---\nafter\n";
    let lines = lines_for(content, content.find("after").unwrap(), true);
    assert_eq!(joined_text(&lines[0], content), "");
    let decor = lines[0]
        .decor
        .as_ref()
        .expect("a Rendered hr line must carry decor");
    assert!(decor.is_rule, "hr decor must be marked as a rule");
}

// --- (b) decor present iff Rendered -----------------------------------

#[test]
fn heading_decor_present_only_while_concealed() {
    let content = "# Title\nbody\n";
    let concealed = lines_for(content, content.find("body").unwrap(), true);
    assert!(
        concealed[0].decor.is_some(),
        "concealed (Rendered) heading must carry decor"
    );

    let revealed = lines_for(content, 0, true); // cursor on the heading line
    assert!(
        revealed[0].decor.is_none(),
        "revealed heading must carry NO decor"
    );
}

#[test]
fn list_item_decor_present_only_while_concealed() {
    let content = "- item\n";
    let concealed = lines_for(content, content.len(), false);
    assert!(concealed[0].decor.is_some());

    let revealed = lines_for(content, 0, true); // cursor on the item's own line
    assert!(revealed[0].decor.is_none());
}

#[test]
fn quote_marker_decor_present_only_while_concealed() {
    let content = "> q\n";
    let concealed = lines_for(content, content.len(), false);
    assert!(concealed[0].decor.is_some());

    let revealed = lines_for(content, 0, true);
    assert!(revealed[0].decor.is_none());
}

#[test]
fn hr_decor_present_only_while_concealed() {
    let content = "---\nafter\n";
    let concealed = lines_for(content, content.find("after").unwrap(), true);
    assert!(concealed[0].decor.is_some());

    let revealed = lines_for(content, 0, true); // cursor on the hr line
    assert!(revealed[0].decor.is_none());
}

// --- (c) task items keep the checkbox glyph and get no bullet decor ---

#[test]
fn task_item_keeps_checkbox_glyph_and_carries_no_bullet_decor() {
    let content = "- [ ] todo\n";
    let lines = lines_for(content, content.len(), false); // unfocused: Rendered
    let joined = joined_text(&lines[0], content);
    assert!(
        joined.contains('\u{2610}'),
        "unchecked task item must still substitute the ☐ glyph: {joined:?}"
    );
    assert!(
        lines[0].decor.is_none(),
        "a task item must carry NO bullet decor even while Rendered"
    );

    // A checked task item substitutes the ☑ glyph and is likewise
    // undecorated.
    let checked = "- [x] done\n";
    let lines = lines_for(checked, checked.len(), false);
    let joined = joined_text(&lines[0], checked);
    assert!(
        joined.contains('\u{2611}'),
        "checked item must show ☑: {joined:?}"
    );
    assert!(lines[0].decor.is_none());
}

#[test]
fn task_checkbox_span_is_still_substituted_not_identical() {
    let content = "- [ ] todo\n";
    let lines = lines_for(content, content.len(), false);
    let has_substituted_checkbox = lines[0]
        .spans
        .iter()
        .any(|s| matches!(s, SyntaxSpan::Substituted { text, .. } if text == "\u{2610}"));
    assert!(
        has_substituted_checkbox,
        "checkbox must ride a Substituted span"
    );
}

// --- nesting shapes: depth-cycled bullets, stacked quote bars ----------

#[test]
fn nested_bullet_uses_a_different_glyph_than_its_parent_depth() {
    let content = "- top\n  - nested\n";
    let lines = lines_for(content, content.len(), false); // unfocused: everything Rendered
    let top_glyph = &lines[0].decor.as_ref().expect("top item decor").pieces[0].first;
    let nested_glyph = &lines[1].decor.as_ref().expect("nested item decor").pieces[0].first;
    assert_ne!(
        top_glyph, nested_glyph,
        "depth 0 and depth 1 bullets must cycle to different glyphs"
    );
}

#[test]
fn nested_blockquote_stacks_one_bar_piece_per_marker_outermost_first() {
    let content = "> > q\n";
    let lines = lines_for(content, content.len(), false); // unfocused: Rendered
    let decor = lines[0]
        .decor
        .as_ref()
        .expect("nested quote must carry decor");
    assert_eq!(
        decor.pieces.len(),
        2,
        "one bar piece per nesting level, outer then inner"
    );
}

// --- a heading leading a list item wins over the item's own bullet ----

#[test]
fn atx_heading_leading_a_list_item_suppresses_the_bullet() {
    for content in ["- # h\n", "- ## h\n"] {
        let lines = lines_for(content, content.len(), false); // unfocused: Rendered
        let decor = lines[0]
            .decor
            .as_ref()
            .expect("the heading icon must still decorate the row");
        assert_eq!(
            decor.pieces.len(),
            1,
            "only the heading icon piece, no bullet, for {content:?}"
        );
    }
}

#[test]
fn ordered_heading_leading_a_list_item_suppresses_the_bullet() {
    let content = "1. # h\n";
    let lines = lines_for(content, content.len(), false);
    let decor = lines[0]
        .decor
        .as_ref()
        .expect("the heading icon must still decorate the row");
    assert_eq!(decor.pieces.len(), 1, "no bullet piece stacked on top");
}

#[test]
fn heading_on_a_later_row_than_the_marker_leaves_the_bullet_intact() {
    let content = "- text\n\n  # h\n";
    let lines = lines_for(content, content.len(), false);
    let marker_decor = lines[0]
        .decor
        .as_ref()
        .expect("the marker row must keep its bullet");
    assert_eq!(marker_decor.pieces.len(), 1);
    assert!(
        !marker_decor.pieces[0].first.contains('#'),
        "the marker row's piece must be the bullet, not a heading glyph"
    );

    let heading_row = lines
        .iter()
        .position(|l| joined_text(l, content).contains('h'))
        .expect("the heading's own row");
    assert_ne!(
        heading_row, 0,
        "the heading must sit on its own row, not the marker's"
    );
    assert!(
        lines[heading_row].decor.is_some(),
        "the heading's own row still carries its icon decor"
    );
}

#[test]
fn task_item_is_unaffected_by_the_heading_wins_rule() {
    let content = "- [ ] x\n";
    let lines = lines_for(content, content.len(), false);
    assert!(
        lines[0].decor.is_none(),
        "a task item still carries no bullet decor"
    );
}

#[test]
fn plain_list_item_keeps_its_bullet_control() {
    let content = "- text\n";
    let lines = lines_for(content, content.len(), false);
    assert!(lines[0].decor.is_some(), "a plain item keeps its bullet");
}

#[test]
fn nested_list_items_each_keep_their_own_depth_bullet_control() {
    let content = "- a\n  - b\n";
    let lines = lines_for(content, content.len(), false);
    assert!(lines[0].decor.is_some());
    assert!(lines[1].decor.is_some());
}

#[test]
fn hr_inside_a_blockquote_keeps_the_bar_piece_ahead_of_the_rule() {
    let content = "> ---\n";
    let lines = lines_for(content, content.len(), false);
    let decor = lines[0]
        .decor
        .as_ref()
        .expect("a quoted thematic break must carry decor");
    assert!(
        decor.is_rule,
        "the combined decor must still be a rule line"
    );
    assert!(
        decor.pieces.len() >= 2,
        "the quote bar must survive the rule producer, not be clobbered"
    );
    let bar = &decor.pieces[0].first;
    assert!(
        !bar.contains('\u{2500}'),
        "the first piece must be the quote bar, not the rule"
    );
    assert!(
        decor.pieces[1].first.contains('\u{2500}'),
        "the rule piece follows the bar"
    );
}
