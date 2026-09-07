use super::history::fuzzy_filter;
use super::test_support::*;
use super::*;

fn with_history(app: &mut App, entries: &[&str]) {
    app.find_mut().expect("panel open").find.history =
        entries.iter().map(|entry| entry.to_string()).collect();
}

#[test]
fn fuzzy_filter_is_case_insensitive_subsequence_preserving_mru_order() {
    let history = vec![
        "Readme Notes".to_string(),
        "todo list".to_string(),
        "REDO stack".to_string(),
    ];
    let hits: Vec<&str> = fuzzy_filter(&history, "rdo")
        .into_iter()
        .map(String::as_str)
        .collect();
    assert_eq!(hits, vec!["Readme Notes", "REDO stack"]);
}

#[test]
fn fuzzy_filter_empty_draft_returns_everything_unfiltered() {
    let history = vec!["a".to_string(), "b".to_string()];
    let hits: Vec<&str> = fuzzy_filter(&history, "")
        .into_iter()
        .map(String::as_str)
        .collect();
    assert_eq!(hits, vec!["a", "b"]);
}

#[test]
fn up_filters_history_against_the_currently_typed_draft() {
    let mut app = app_with("hello");
    open_find(&mut app);
    with_history(&mut app, &["needle", "hay", "haystack"]);
    type_str(&mut app, "ha");

    press(&mut app, up());

    assert_eq!(find(&app).find.draft, "hay");
}

#[test]
fn up_walks_older_in_mru_order_and_clamps_at_the_oldest() {
    let mut app = app_with("hello");
    open_find(&mut app);
    with_history(&mut app, &["one", "two"]);

    press(&mut app, up());
    assert_eq!(find(&app).find.draft, "one");
    press(&mut app, up());
    assert_eq!(find(&app).find.draft, "two");
    press(&mut app, up());
    assert_eq!(find(&app).find.draft, "two");
}

#[test]
fn down_past_the_newest_entry_restores_the_in_progress_draft() {
    let mut app = app_with("hello");
    open_find(&mut app);
    with_history(&mut app, &["hello world", "help"]);
    type_str(&mut app, "h");

    press(&mut app, up());
    assert_eq!(find(&app).find.draft, "hello world");

    press(&mut app, down());
    assert_eq!(
        find(&app).find.draft,
        "h",
        "walking down past the newest entry restores the pre-browse draft"
    );
    assert!(find(&app).find.history_pos.is_none());
}

#[test]
fn down_with_no_browse_session_active_is_a_no_op() {
    let mut app = app_with("hello");
    open_find(&mut app);
    with_history(&mut app, &["one"]);
    type_str(&mut app, "x");

    press(&mut app, down());
    assert_eq!(find(&app).find.draft, "x");
}

#[test]
fn typing_after_browsing_history_resets_the_browse_session() {
    let mut app = app_with("hello");
    open_find(&mut app);
    with_history(&mut app, &["one"]);
    press(&mut app, up());
    assert_eq!(find(&app).find.draft, "one");

    press(&mut app, char_key('!'));
    assert_eq!(find(&app).find.draft, "one!");
    assert!(find(&app).find.history_pos.is_none());
}

#[test]
fn recalling_a_history_entry_follows_it_live() {
    let mut app = app_with("alpha beta");
    open_find(&mut app);
    with_history(&mut app, &["beta"]);

    press(&mut app, up());

    assert_eq!(find(&app).matches, vec![6..10]);
    assert_eq!(selection_start(&app), 6);
}

#[test]
fn up_on_a_chip_reports_instead_of_browsing() {
    let mut app = app_with("hello");
    open_find(&mut app);
    with_history(&mut app, &["one"]);
    press(&mut app, tab());

    press(&mut app, up());

    assert_eq!(find(&app).find.draft, "");
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("press \u{2423} to toggle")
    );
}
