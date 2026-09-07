use crate::app::App;
use crate::find::test_support::*;
use crate::keymap::KeyCode;
use crate::render::{RowSource, build_rows};
use crate::width::display_width;

fn preview_for(content: &str, needle: &str, replacement: &str) -> App {
    let mut app = app_with(content);
    open_find(&mut app);
    type_str(&mut app, needle);
    open_replace(&mut app);
    type_str(&mut app, replacement);
    app
}

#[test]
fn typing_in_replace_previews_every_visible_match_without_editing() {
    let mut app = preview_for("dog cat dog cat dog", "dog", "bird");
    let version = app.active_doc().buffer.version();

    let (_, row) = document_row(&mut app, "bird cat bird cat bird");

    assert!(!row.contains("dog"), "{row:?}");
    assert_eq!(app.active_doc().buffer.content(), "dog cat dog cat dog");
    assert_eq!(app.active_doc().buffer.version(), version);
}

#[test]
fn the_current_match_preview_is_tinted_stronger_than_the_others() {
    let mut app = preview_for("dog cat dog cat dog", "dog", "bird");
    let (y, row) = document_row(&mut app, "bird cat bird cat bird");
    let xs = occurrences(&row, "bird");
    assert_eq!(xs.len(), 3);
    let current = app.theme.chrome.replace_preview_current_bg.bg;
    let other = app.theme.chrome.replace_preview_bg.bg;

    assert_eq!(bg_at(&mut app, xs[0], y), current);
    assert_eq!(
        bg_at(&mut app, xs[0] + 3, y),
        current,
        "the whole replacement is tinted"
    );
    assert_eq!(bg_at(&mut app, xs[1], y), other);
    assert_eq!(bg_at(&mut app, xs[2], y), other);
}

#[test]
fn unfocusing_the_panel_drops_the_preview_and_keeps_the_match_highlights() {
    let mut app = preview_for("dog cat dog cat dog", "dog", "bird");
    let geo = crate::layout::geometry(app.frame_area(), &app);

    click(&mut app, geo.editor.x, geo.editor.y);

    assert!(app.find().is_some_and(|state| !state.focused));
    let (y, row) = document_row(&mut app, "dog cat dog cat dog");
    let xs = occurrences(&row, "dog");
    let highlight = app.theme.chrome.search_match_bg.bg;
    assert_eq!(bg_at(&mut app, xs[1], y), highlight);
    assert_eq!(bg_at(&mut app, xs[2], y), highlight);
    assert!(!document_rows(&mut app).iter().any(|r| r.contains("bird")));
}

#[test]
fn a_kept_panel_previews_again_once_it_is_refocused() {
    let mut app = preview_for("dog", "dog", "bird");
    let geo = crate::layout::geometry(app.frame_area(), &app);
    click(&mut app, geo.editor.x, geo.editor.y);
    let panel = geo.find_panel.expect("panel open");
    let field = panel.replace_field.expect("replace row shown");

    click(&mut app, field.x, field.y);

    document_row(&mut app, "bird");
}

#[test]
fn a_match_whose_cells_do_not_reproduce_its_bytes_keeps_the_plain_highlight() {
    let mut app = app_with("a\tdog");
    open_find(&mut app);
    type_str(&mut app, r"\sdog");
    press(&mut app, key(KeyCode::Char('r'), ALT));
    assert_eq!(app.find().expect("open").matches, vec![1..5]);
    open_replace(&mut app);
    type_str(&mut app, "bird");

    let rows = document_rows(&mut app);

    assert!(rows.iter().any(|r| r.contains("dog")), "{rows:?}");
    assert!(!rows.iter().any(|r| r.contains("bird")), "{rows:?}");
}

#[test]
fn a_match_inside_a_boxed_table_keeps_the_plain_highlight() {
    let mut app = app_with("| dog | b |\n|-----|---|\n| c | d |\n");
    press(&mut app, key(KeyCode::Char('P'), CTRL));
    assert!(
        app.active_doc().is_read_only(),
        "reading view keeps the table boxed"
    );
    open_find(&mut app);
    type_str(&mut app, "dog");
    open_replace(&mut app);
    type_str(&mut app, "bird");
    let rows = document_rows(&mut app);
    let boxed = rows.iter().find(|row| row.contains("dog"));
    assert!(boxed.is_some(), "the table row is on screen: {rows:?}");

    assert!(
        boxed.is_some_and(|row| row.contains('\u{2502}')),
        "the table is drawn boxed: {boxed:?}"
    );
    assert!(!rows.iter().any(|r| r.contains("bird")), "{rows:?}");
}

fn body(row: &str) -> &str {
    row.trim_start_matches('\u{2502}')
        .trim_end()
        .trim_end_matches('\u{2502}')
}

#[test]
fn a_match_wrapped_across_two_rows_keeps_the_plain_highlight() {
    let width = app_with("").active_doc().viewport.width as usize;
    let content = format!("{}dog{}", "x".repeat(width - 1), "x".repeat(3));
    let mut app = preview_for(&content, "dog", "bird");
    let rows = document_rows(&mut app);
    let split = rows.iter().position(|row| body(row).ends_with("xd"));
    assert!(
        split.is_some(),
        "the match must break after its first cell: {rows:?}"
    );
    let y = split.unwrap_or_default();

    assert!(body(&rows[y + 1]).starts_with("og"), "{rows:?}");
    assert!(!rows.iter().any(|r| r.contains("bird")), "{rows:?}");
}

#[test]
fn a_zero_width_joiner_in_the_replacement_never_yields_a_cell_ratatui_would_widen() {
    let mut app = preview_for("dog", "dog", "\u{200d}ab");
    app.sync_view();
    let view = app.active_doc().view.clone().expect("a synced view");

    let rows = build_rows(&app, RowSource::Shown, &view);

    let previewed: String = rows
        .iter()
        .flatten()
        .filter(|cell| cell.buf_offset == Some(0))
        .map(|cell| cell.text.as_str())
        .collect();
    assert_eq!(previewed, "\u{200d}ab");
    for cell in rows.iter().flatten().filter(|c| c.buf_offset.is_some()) {
        if cell.width == 0 {
            assert_eq!(display_width(&cell.text), 0, "{:?}", cell.text);
        }
    }
}

#[test]
fn an_empty_replacement_previews_the_deletion() {
    let mut app = preview_for("dog cat dog", "dog", "");

    let (_, row) = document_row(&mut app, " cat ");

    assert!(!row.contains("dog"), "{row:?}");
}

#[test]
fn a_replacement_wider_than_the_match_is_clipped_at_the_row_edge_not_wrapped() {
    let width = app_with("").active_doc().viewport.width as usize;
    let content = format!("{}dog", "x".repeat(width - 3));
    let mut app = preview_for(&content, "dog", "elephant");
    let rows = document_rows(&mut app);
    let start = rows.iter().position(|row| row.contains("xxxele"));
    assert!(
        start.is_some(),
        "the preview starts where the match was: {rows:?}"
    );
    let y = start.unwrap_or_default();

    assert!(body(&rows[y]).ends_with("ele"), "{:?}", rows[y]);
    assert!(!rows[y + 1].contains("phant"), "{:?}", rows[y + 1]);
}

#[test]
fn a_stale_match_set_never_previews_onto_a_newer_buffer() {
    let mut app = preview_for("dog cat dog", "dog", "bird");
    let geo = crate::layout::geometry(app.frame_area(), &app);
    click(&mut app, geo.editor.x, geo.editor.y);
    press(&mut app, char_key('q'));
    assert_eq!(app.active_doc().buffer.content(), "qdog cat dog");
    let field = geo
        .find_panel
        .and_then(|p| p.replace_field)
        .expect("replace row");

    click(&mut app, field.x, field.y);
    let rows = document_rows(&mut app);

    assert!(
        rows.iter().any(|r| r.contains("qbird cat bird")),
        "{rows:?}"
    );
}
