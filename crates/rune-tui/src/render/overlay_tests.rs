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

#[test]
fn a_span_starting_past_the_visible_window_is_excluded() {
    let theme = Theme::catppuccin_mocha(false);
    let plain = Style::default();
    let mut rows = vec![(0..5).map(cell).collect::<Vec<Cell>>()];
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

#[test]
fn all_decorative_cells_leave_rows_untouched() {
    let theme = Theme::catppuccin_mocha(false);
    let mut rows = vec![vec![decorative_cell(), decorative_cell()]];
    let before = rows.clone();
    apply_highlight_spans(&mut rows, &[(0..2, scope("function"))], &theme);
    assert_eq!(rows, before);
}

// A full end-to-end session can no longer reach this branch with a caret
// actually on screen (the caret gate and a table's own boxed decision key
// off near-identical predicates), so the clamp keeps its own direct unit
// coverage here.
#[test]
fn place_caret_clamps_onto_a_boxed_rows_last_cell_instead_of_appending() {
    let mut row: Vec<Cell> = (0..3).map(cell).collect();
    let before_len = row.len();
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
