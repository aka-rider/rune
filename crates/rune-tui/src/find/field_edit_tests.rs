use super::test_support::*;
use super::*;
use crate::runtime::Msg;

fn paste(app: &mut App, text: &str) {
    let mut effects = Effects::default();
    crate::app::update(app, Msg::Paste(text.to_string()), &mut effects);
}

#[test]
fn typing_recomputes_matches_live() {
    let mut app = app_with("hello world hello");
    open_find(&mut app);
    type_str(&mut app, "hello");
    let state = find(&app);
    assert_eq!(state.find.editor.text(), "hello");
    assert_eq!(state.matches, vec![0..5, 12..17]);
}

#[test]
fn backspace_on_an_empty_draft_leaves_the_panel_open() {
    let mut app = app_with("hello");
    open_find(&mut app);
    press(&mut app, backspace());
    assert!(
        app.find().is_some(),
        "an empty-draft Backspace must not close the panel"
    );
}

#[test]
fn backspace_erases_one_grapheme_and_refollows() {
    let mut app = app_with("ab ab");
    open_find(&mut app);
    type_str(&mut app, "ab");
    assert_eq!(find(&app).matches, vec![0..2, 3..5]);

    press(&mut app, backspace());
    let state = find(&app);
    assert_eq!(state.find.editor.text(), "a");
    assert_eq!(state.matches, vec![0..1, 3..4]);
    assert_eq!(state.current, Some(0));
}

#[test]
fn backspace_in_the_middle_of_the_text_deletes_left_of_the_caret_not_at_the_end() {
    let mut app = app_with("hello");
    open_find(&mut app);
    type_str(&mut app, "abd");
    press(&mut app, left());
    press(&mut app, left());

    press(&mut app, backspace());

    assert_eq!(
        find(&app).find.editor.text(),
        "bd",
        "the caret sat between 'a' and 'b', so backspace removes the 'a' before it"
    );
    assert_eq!(find(&app).find.editor.cursor().position.get(), 0);
}

#[test]
fn left_twice_then_shift_right_then_typing_replaces_the_selected_character() {
    let mut app = app_with("hello");
    open_find(&mut app);
    type_str(&mut app, "cat");
    press(&mut app, left());
    press(&mut app, left());
    press(&mut app, shift_right());
    assert_eq!(find(&app).find.editor.selected_text(), "a");

    press(&mut app, char_key('u'));

    assert_eq!(find(&app).find.editor.text(), "cut");
}

#[test]
fn alt_left_moves_the_caret_to_the_word_start() {
    let mut app = app_with("hello");
    open_find(&mut app);
    type_str(&mut app, "foo bar");
    assert_eq!(find(&app).find.editor.cursor().position.get(), 7);

    press(&mut app, alt_left());

    assert_eq!(find(&app).find.editor.cursor().position.get(), 4);
}

#[test]
fn alt_b_moves_the_caret_to_the_word_start() {
    let mut app = app_with("hello");
    open_find(&mut app);
    type_str(&mut app, "foo bar");

    press(&mut app, alt_b());

    assert_eq!(find(&app).find.editor.cursor().position.get(), 4);
}

#[test]
fn escape_closes_the_panel_and_saves_the_query() {
    let mut app = app_with("hello");
    open_find(&mut app);
    type_str(&mut app, "h");
    press(&mut app, esc_key());
    assert!(app.find().is_none(), "Escape closes the panel");
    assert_eq!(
        app.last_find.as_ref().map(|(query, _)| query.as_str()),
        Some("h")
    );
}

#[test]
fn arrow_keys_with_empty_history_leave_the_draft_untouched() {
    let mut app = app_with("hello");
    open_find(&mut app);
    type_str(&mut app, "h");
    press(&mut app, up());
    assert_eq!(find(&app).find.editor.text(), "h");
    press(&mut app, down());
    assert_eq!(find(&app).find.editor.text(), "h");
}

#[test]
fn typing_in_the_replace_field_never_moves_the_cursor_or_touches_the_buffer() {
    let mut app = app_with("dog dog");
    open_find(&mut app);
    type_str(&mut app, "dog");
    open_replace(&mut app);
    let before = app.active_doc().buffer().content().to_string();
    type_str(&mut app, "cat");

    assert_eq!(app.replace_draft(), Some("cat"));
    assert_eq!(find(&app).find.editor.text(), "dog");
    assert_eq!(app.active_doc().buffer().content(), before);
    assert_eq!(selection_start(&app), 0);
}

#[test]
fn paste_appends_to_the_draft_and_never_touches_the_buffer() {
    let mut app = app_with("hello world");
    open_find(&mut app);
    let before = app.active_doc().buffer().content().to_string();

    paste(&mut app, "wor\nld");

    assert_eq!(find(&app).find.editor.text(), "wor");
    assert_eq!(find(&app).matches, vec![6..9]);
    assert_eq!(app.active_doc().buffer().content(), before);
}

#[test]
fn paste_with_the_caret_in_the_middle_inserts_there() {
    let mut app = app_with("hello");
    open_find(&mut app);
    type_str(&mut app, "hlo");
    press(&mut app, left());
    press(&mut app, left());

    paste(&mut app, "el");

    assert_eq!(find(&app).find.editor.text(), "hello");
}

#[test]
fn paste_over_a_selection_replaces_it() {
    let mut app = app_with("hello");
    open_find(&mut app);
    type_str(&mut app, "hello");
    press(&mut app, left());
    press(&mut app, left());
    press(&mut app, left());
    press(&mut app, left());
    press(&mut app, left());
    press(&mut app, shift_right());
    press(&mut app, shift_right());
    press(&mut app, shift_right());
    press(&mut app, shift_right());
    press(&mut app, shift_right());
    assert_eq!(find(&app).find.editor.selected_text(), "hello");

    paste(&mut app, "bye");

    assert_eq!(find(&app).find.editor.text(), "bye");
}

#[test]
fn paste_strips_control_characters() {
    let mut app = app_with("hello");
    open_find(&mut app);
    paste(&mut app, "a\u{7}b");
    assert_eq!(find(&app).find.editor.text(), "ab");
}

#[test]
fn paste_lands_in_the_replace_field_when_it_holds_the_ring() {
    let mut app = app_with("hello");
    open_replace(&mut app);
    paste(&mut app, "term");
    assert_eq!(app.replace_draft(), Some("term"));
    assert_eq!(find(&app).find.editor.text(), "");
}

#[test]
fn paste_with_a_kept_panel_goes_to_the_document() {
    let mut app = app_with("hello");
    open_find(&mut app);
    crate::find::accept_location(&mut app);

    paste(&mut app, "term");

    assert_eq!(find(&app).find.editor.text(), "");
    assert_eq!(app.active_doc().buffer().content(), "termhello");
}
