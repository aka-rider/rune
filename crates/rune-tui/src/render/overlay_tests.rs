use super::*;
use crate::theme::Theme;
use rune_syntax::scope::scope_table;

fn cell(offset: u32) -> Cell {
    Cell {
        text: "x".into(),
        width: 1,
        style: Style::default(),
        buf_offset: Some(offset),
    }
}

fn decorative_cell() -> Cell {
    Cell {
        buf_offset: None,
        ..cell(0)
    }
}

fn scope(name: &str) -> ScopeId {
    scope_table().resolve(name).expect("known scope name")
}

/// An outer span painted first, then a nested span painted
/// over it, leaves the nested bytes with the INNER style and everything
/// else with the OUTER one — innermost-wins, no per-cell search.
#[test]
fn nested_span_overwrites_the_outer_one_it_sits_inside() {
    let theme = Theme::catppuccin_mocha(false);
    let function_style = theme.scope_style(scope("function"));
    let variable_style = theme.scope_style(scope("variable"));

    let mut rows = vec![(0..10).map(cell).collect::<Vec<Cell>>()];
    let spans = vec![(0..10, scope("function")), (3..5, scope("variable"))];
    apply_highlight_spans(&mut rows, &spans, &theme);

    let row = &rows[0];
    for i in [0, 1, 2, 5, 6, 7, 8, 9] {
        assert_eq!(
            row[i].style, function_style,
            "cell {i} should be function-styled"
        );
    }
    for i in [3, 4] {
        assert_eq!(
            row[i].style, variable_style,
            "cell {i} should be variable-styled"
        );
    }
}

/// The overlay patches `style` only — every cell's `buf_offset`/`width`
/// must come out byte-identical to what went in.
#[test]
fn overlay_changes_style_only_never_offset_or_width() {
    let theme = Theme::catppuccin_mocha(false);
    let before: Vec<Cell> = (0..10).map(cell).collect();
    let mut rows = vec![before.clone()];
    let spans = vec![(0..10, scope("function")), (3..5, scope("variable"))];
    apply_highlight_spans(&mut rows, &spans, &theme);

    for (b, a) in before.iter().zip(rows[0].iter()) {
        assert_eq!(b.buf_offset, a.buf_offset);
        assert_eq!(b.width, a.width);
    }
}

/// A span whose `start` sits past the visible window
/// (`hi`) must still be excluded now that the window scan cuts off at
/// `partition_point(start < hi)` instead of scanning every span — this
/// pins that the cut doesn't accidentally paint (or panic on) a span
/// that used to just be skipped by the old `range.start >= hi` filter.
#[test]
fn a_span_starting_past_the_visible_window_is_excluded() {
    let theme = Theme::catppuccin_mocha(false);
    let plain = Style::default();
    let mut rows = vec![(0..5).map(cell).collect::<Vec<Cell>>()];
    // Sorted by `start` ASC (painter order): one span inside the
    // window, one entirely past it.
    let spans = vec![(1..3, scope("variable")), (100..200, scope("function"))];
    apply_highlight_spans(&mut rows, &spans, &theme);

    let row = &rows[0];
    assert_eq!(row[0].style, plain);
    assert_eq!(row[1].style, theme.scope_style(scope("variable")));
    assert_eq!(row[2].style, theme.scope_style(scope("variable")));
    for i in [3, 4] {
        assert_eq!(row[i].style, plain, "cell {i} is outside every span");
    }
}

/// A span that starts before the window's `hi` but extends past `lo`'s
/// window into the document tail must still paint its portion INSIDE
/// the window — the `partition_point` cut is on `start`, not `end`, so
/// a wide span isn't dropped just because it outlives the window.
#[test]
fn a_span_straddling_the_window_boundary_still_paints_its_visible_portion() {
    let theme = Theme::catppuccin_mocha(false);
    let mut rows = vec![(0..5).map(cell).collect::<Vec<Cell>>()];
    let spans = vec![(2..1000, scope("function"))];
    apply_highlight_spans(&mut rows, &spans, &theme);

    let row = &rows[0];
    assert_eq!(row[0].style, Style::default());
    assert_eq!(row[1].style, Style::default());
    for i in [2, 3, 4] {
        assert_eq!(row[i].style, theme.scope_style(scope("function")));
    }
}

/// No visible (non-decorative) cell means nothing to paint — the window
/// scan returns early and `rows` is left exactly as it was.
#[test]
fn all_decorative_cells_leave_rows_untouched() {
    let theme = Theme::catppuccin_mocha(false);
    let mut rows = vec![vec![decorative_cell(), decorative_cell()]];
    let before = rows.clone();
    apply_highlight_spans(&mut rows, &[(0..2, scope("function"))], &theme);
    assert_eq!(rows, before);
}

/// The `TABLE-ROW-WIDTH` regression `place_caret`'s `boxed` branch
/// exists for (`crates/rune-fuzz/proptest-regressions/human_session.txt`,
/// seed `cc 5f23e392...`), exercised directly rather than through the
/// full `App`/`DocMachine` pipeline: the caret gate this file's
/// `apply_cursor_overlays` applies (its `caret` parameter) and a table's
/// `RevealGrant::ForceRendered`/`Decide` split
/// (`rune_md::element::doc::DocMachine`) key off near-identical
/// predicates (`Document::has_insertion_point` for the caret,
/// `Document::reveals_under_cursor` for reveal — they differ only while
/// the search bar is open, when the caret blurs but reveal stays live)
/// — a table containing the cursor is only ever BOXED while reveal is
/// disengaged, and the caret gate suppresses painting under at least
/// that same condition, so a full-pipeline test can no longer reach
/// this branch with a caret actually on screen. The clamp logic itself is still real (a
/// non-markdown pathway, or a future Decide policy change, could still
/// reach a boxed row with the caret visible), so it keeps its own
/// direct coverage here instead.
#[test]
fn place_caret_clamps_onto_a_boxed_rows_last_cell_instead_of_appending() {
    let mut row: Vec<Cell> = (0..3).map(cell).collect();
    let before_len = row.len();
    // Far past the row's own rendered width (3 cells) — the ragged-row
    // case's dropped trailing `|`-cells produce exactly this: a
    // `visual_col` past every real cell on a boxed row.
    place_caret(&mut row, 100, 0, true);
    assert_eq!(
        row.len(),
        before_len,
        "a boxed row must never grow a cell wider from the caret clamp"
    );
    assert!(
        row.last()
            .is_some_and(|c| c.style.add_modifier.contains(RtModifier::REVERSED)),
        "the clamp must still reverse-video the row's own last cell"
    );
}

/// The unboxed counterpart: an ordinary (non-table) row past its last
/// visible char DOES grow by one synthetic EOL cursor cell — the
/// `TABLE-ROW-WIDTH` exemption is boxed-rows-only.
#[test]
fn place_caret_appends_a_synthetic_eol_cell_on_an_unboxed_row() {
    let mut row: Vec<Cell> = (0..3).map(cell).collect();
    place_caret(&mut row, 100, 7, false);
    assert_eq!(
        row.len(),
        4,
        "an unboxed row past its last cell must grow by one"
    );
    assert!(
        row.last()
            .is_some_and(|c| c.style.add_modifier.contains(RtModifier::REVERSED)),
        "the appended synthetic cell must carry the caret's reverse-video"
    );
}
