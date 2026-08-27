//! WP4: end-to-end Pivoted-layout table rendering through the real
//! pipeline (parse -> sync_cursors -> emit). Split from the combined
//! `table_render` file into per-layout groups — this one is
//! Pivoted.
//!
//! Every "Rendered" assertion below uses `focused = false`: an unfocused
//! document forces every Decide-policy block Rendered regardless of cursor
//! position (`DocMachine::sync_cursors`'s `RevealGrant::ForceRendered`
//! root grant) — simpler than hunting for a cursor offset genuinely
//! outside a table whose every line IS the table.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod table_render_common;

use rune_md::emit::emit;
use rune_md::table::layout::{TableLayout, choose};
use table_render_common::{display_rows_at, joined_line, synced, wrap_pivot_url};

/// WP4.S7: the SAME table collapses to Pivoted at width 20 (verified
/// against `choose`'s own thresholds: every column is atomic-dominant once
/// the frame overhead eats most of the tiny content budget). The body row
/// becomes one `"  Label: Value"` row per column — `"Name: "` must appear,
/// and no `│` anywhere (Pivoted abandons the box shape entirely).
#[test]
fn pivoted_layout_renders_label_value_pairs_with_no_box_drawing() {
    let content = wrap_pivot_url();
    let (buf, doc) = synced(&content, 0, false);
    let width = 20u16;
    let (lines, _snap) = emit(buf.content(), doc.blocks(), width);

    let widths = vec![5usize, 35, 65];
    let min_widths = vec![5usize, 11, 65];
    assert_eq!(
        choose(&widths, &min_widths, width as usize),
        TableLayout::Pivoted
    );

    let body_line = 2;
    let info = lines[body_line].table.as_ref().expect("rendered table row");

    let mut all_text = joined_line(&lines, body_line, buf.content());
    for extra in &info.extra_rows {
        all_text.push_str(
            &extra
                .iter()
                .map(|s| s.text(buf.content()))
                .collect::<String>(),
        );
    }
    assert!(
        all_text.contains("Name: "),
        "expected a Name label:value pair"
    );
    assert!(!all_text.contains('│'), "Pivoted never draws a box");

    // Header and separator lines are suppressed to blank under Pivoted.
    assert!(joined_line(&lines, 0, buf.content()).is_empty());
    assert!(joined_line(&lines, 1, buf.content()).is_empty());
}

/// A Pivoted table draws no box, so the display pass must not synthesise
/// `┌┬┐`/`├┼┤`/`└┴┘` rows around it. Asserted at the DISPLAY level rather
/// than on spans: border rows never appear in a `SyntaxLine`'s spans at
/// all, so a span-level "no box drawing" check passes whether or not the
/// expansion pass is doing the right thing.
#[test]
fn pivoted_table_gets_no_synthetic_border_rows_in_the_display_snapshot() {
    let content = wrap_pivot_url();
    let (buf, doc) = synced(&content, 0, false);

    let grid = display_rows_at(&buf, &doc, 200);
    let pivot = display_rows_at(&buf, &doc, 20);

    let boxy = |rows: &[String]| {
        rows.iter()
            .filter(|r| r.starts_with('┌') || r.starts_with('└') || r.starts_with('├'))
            .count()
    };

    assert!(
        boxy(&grid) > 0,
        "a Grid table must still get border rows: {grid:#?}"
    );
    assert_eq!(
        boxy(&pivot),
        0,
        "a Pivoted table must get no synthesised border rows: {pivot:#?}"
    );
}

/// `emit_table` derives which body row is the FIRST by document order
/// (`first_body_line`) and only skips the leading `─` rule for THAT one —
/// every later body row's own source line must instead render the rule as
/// its row-1 text (the label:value pairs move to `extra_rows` behind it).
/// Two body rows exercise both sides at once: get `first_body_line`
/// pointed at the header's own line instead of the first body row's (or
/// invert the `!=` that compares against it), and either the first row
/// wrongly grows a leading rule, the second wrongly loses its own, or
/// both.
#[test]
fn only_the_first_pivoted_body_row_skips_its_own_leading_rule() {
    let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n";
    let (buf, doc) = synced(content, 0, false);
    let width = 8u16;
    let (lines, _snap) = emit(buf.content(), doc.blocks(), width);
    assert_eq!(
        choose(&[5, 3], &[5, 3], width as usize),
        TableLayout::Pivoted,
        "fixture must actually collapse to Pivoted at this width"
    );

    let first = joined_line(&lines, 2, buf.content());
    assert!(
        first.contains("Name: Alice") && !first.chars().all(|c| c == '─'),
        "the first record must render its label:value pair directly, no leading rule: {first:?}"
    );

    let second = joined_line(&lines, 3, buf.content());
    assert!(
        !second.is_empty() && second.chars().all(|c| c == '─'),
        "a later record's own source line must render the leading rule, not its label:value pairs: {second:?}"
    );
    let info = lines[3].table.as_ref().expect("rendered table row");
    let extra_text: String = info
        .extra_rows
        .iter()
        .flat_map(|r| r.iter().map(|s| s.text(buf.content())))
        .collect();
    assert!(
        extra_text.contains("Name: Bob"),
        "the second record's own label:value pairs must still appear, just pushed into extra_rows: {extra_text:?}"
    );
}
