use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_vfs::Mem;

use super::follow::{current_concealed, is_concealed};
use super::test_support::*;
use super::*;
use crate::keymap::GlobalCommand;

fn global(app: &mut App, cmd: GlobalCommand) {
    let mut effects = Effects::default();
    crate::pane::handle_global_command(app, cmd, &mut effects);
}

#[test]
fn enter_wraps_from_the_last_match_to_the_first() {
    let mut app = app_with("hi hi hi");
    open_find(&mut app);
    type_str(&mut app, "hi");
    assert_eq!(find(&app).matches, vec![0..2, 3..5, 6..8]);
    assert_eq!(find(&app).current, Some(0));

    press(&mut app, enter());
    assert_eq!(find(&app).current, Some(1));
    press(&mut app, enter());
    assert_eq!(find(&app).current, Some(2));
    assert_eq!(selection_start(&app), 6);
    press(&mut app, enter());
    assert_eq!(find(&app).current, Some(0));
    assert_eq!(selection_start(&app), 0);
}

#[test]
fn shift_enter_wraps_from_the_first_match_to_the_last() {
    let mut app = app_with("hi hi hi");
    open_find(&mut app);
    type_str(&mut app, "hi");

    press(&mut app, shift_enter());
    assert_eq!(find(&app).current, Some(2));
    assert_eq!(selection_start(&app), 6);
}

#[test]
fn enter_with_zero_matches_keeps_the_origin_and_says_so() {
    let mut app = app_with("hello");
    open_find(&mut app);
    type_str(&mut app, "zzz");
    assert!(find(&app).matches.is_empty());
    let cursor_before = app.active_doc().cursors.primary().position;

    press(&mut app, enter());
    assert_eq!(find(&app).current, None);
    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("no matches for \"zzz\""),
        "a query with no matches must say so, not fail silently"
    );
}

#[test]
fn matches_fully_inside_a_concealed_table_separator_are_counted_but_skipped() {
    let mut app = app_with("text\n\n| a | b |\n|---|---|\n| a | c |\n");
    open_find(&mut app);
    press(&mut app, char_key('-'));

    let matches = find(&app).matches.clone();
    assert!(!matches.is_empty(), "N still counts every '-'");
    let concealed = current_concealed(&app);
    assert!(
        matches.iter().all(|m| is_concealed(&concealed, m)),
        "every '-' sits inside the substituted separator row"
    );
    assert_eq!(find(&app).current, None, "live follow lands on nothing");
    assert_eq!(selection_start(&app), 0);

    press(&mut app, enter());
    assert_eq!(find(&app).current, None);
    assert_eq!(selection_start(&app), 0);
    assert_eq!(
        crate::messages::newest_text(&app),
        Some(format!("all {} matches are concealed", matches.len())).as_deref(),
        "a concealed-only match list must say so, not fail silently"
    );
}

#[test]
fn revealing_the_table_makes_its_matches_navigable_without_a_buffer_edit() {
    let mut app = app_with("text\n\n| a | b |\n|---|---|\n| a | c |\n");
    open_find(&mut app);
    press(&mut app, char_key('-'));
    let version_before = app.active_doc().buffer.version();
    press(&mut app, enter());
    assert_eq!(
        find(&app).current,
        None,
        "still concealed before the cursor enters the table"
    );

    let table_offset = "text\n\n| a".len();
    app.active_doc_mut().cursors = CursorSet::new(table_offset);
    app.sync_view();
    assert_eq!(
        app.active_doc().buffer.version(),
        version_before,
        "revealing must never look like a buffer edit"
    );

    press(&mut app, enter());
    assert!(
        find(&app).current.is_some(),
        "the revealed row's matches must be navigable on the very next Enter"
    );
}

#[test]
fn a_read_only_document_still_scrolls_to_a_followed_match() {
    let content: String = (0..200).map(|i| format!("line {i} needle\n")).collect();
    let mut app = app_with(&content);
    app.active_doc_mut().read_only = crate::document::ReadOnly::Always;
    app.active_doc_mut().viewport.set_size(80, 10);
    app.sync_view();
    open_find(&mut app);
    let scroll_before = app.active_doc().viewport.scroll_row;

    type_str(&mut app, "line 150");
    app.sync_view();

    assert!(!find(&app).matches.is_empty());
    assert_ne!(
        app.active_doc().viewport.scroll_row,
        scroll_before,
        "a jump on a read-only document must move the viewport explicitly"
    );
}

fn in_memory_db(degraded: bool) -> crate::db::Db {
    crate::db::Db::new(
        rune_db::Store::open_in_memory(
            Arc::new(std::time::SystemTime::now),
            Arc::new(Mem::new()),
            Box::new(|_evt| {}),
        )
        .expect("open in-memory store"),
        crate::db::DbBridge::bootstrap(),
        degraded,
    )
}

#[test]
fn a_degraded_db_attempts_no_write_but_still_navigates() {
    let mut app = app_with("hi hi");
    app.db = Some(in_memory_db(true));
    open_find(&mut app);
    type_str(&mut app, "hi");

    press(&mut app, enter());
    assert_eq!(find(&app).current, Some(1));
    assert_eq!(
        app.last_find,
        Some(("hi".to_string(), MatchOptions::default()))
    );
    assert_eq!(
        crate::messages::newest_text(&app),
        None,
        "a degraded store skips the write entirely, so there is nothing to report"
    );
}

#[test]
fn enter_after_a_coalesced_doc_switch_recomputes_instead_of_jumping_into_the_old_doc() {
    let mut app = app_with("needle needle");
    open_find(&mut app);
    type_str(&mut app, "needle");
    assert_eq!(find(&app).matches, vec![0..6, 7..13]);
    let stale_doc = find(&app).doc;

    let other = app.open_document(Buffer::new("no matches in here"));
    app.active = other;

    press(&mut app, enter());

    let state = find(&app);
    assert_ne!(
        state.doc, stale_doc,
        "the recompute must retarget the new active doc"
    );
    assert_eq!(state.doc, other);
    assert!(state.matches.is_empty());
    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        0,
        "no jump into the wrong document's byte ranges"
    );
}

#[test]
fn repeated_enter_on_the_same_query_enqueues_one_touch_op() {
    let mut app = app_with("hi hi hi");
    app.db = Some(in_memory_db(false));
    open_find(&mut app);
    type_str(&mut app, "hi");

    press(&mut app, enter());
    press(&mut app, enter());
    press(&mut app, enter());

    assert_eq!(
        app.search_history.ops.len(),
        1,
        "an unchanged query across repeated Enter must enqueue exactly one write"
    );
}

#[test]
fn closed_panel_next_steps_and_wraps_using_the_last_query() {
    let mut app = app_with("hi hi hi");
    open_find(&mut app);
    type_str(&mut app, "hi");
    press(&mut app, esc_key());
    assert!(app.find().is_none(), "the panel is closed for this test");
    assert_eq!(
        app.last_find,
        Some(("hi".to_string(), MatchOptions::default()))
    );

    global(&mut app, GlobalCommand::SearchNext);
    assert_eq!(selection_start(&app), 3);
    global(&mut app, GlobalCommand::SearchNext);
    assert_eq!(selection_start(&app), 6);
    global(&mut app, GlobalCommand::SearchNext);
    assert_eq!(
        selection_start(&app),
        0,
        "next wraps from the last match back to the first"
    );
    assert!(
        app.find().is_none(),
        "closed-panel navigation never reopens it"
    );
}

#[test]
fn closed_panel_prev_wraps_from_the_first_match_to_the_last() {
    let mut app = app_with("hi hi hi");
    open_find(&mut app);
    type_str(&mut app, "hi");
    press(&mut app, esc_key());

    global(&mut app, GlobalCommand::SearchPrev);
    assert_eq!(
        selection_start(&app),
        6,
        "prev from the first match wraps to the last"
    );
}

#[test]
fn closed_panel_navigation_keeps_the_match_options() {
    let mut app = app_with("Dog dog dog");
    open_find(&mut app);
    type_str(&mut app, "dog");
    press(&mut app, key(crate::keymap::KeyCode::Char('c'), ALT));
    assert!(find(&app).options.case_sensitive);
    press(&mut app, esc_key());

    global(&mut app, GlobalCommand::SearchNext);
    assert_eq!(selection_start(&app), 4);
    global(&mut app, GlobalCommand::SearchNext);
    assert_eq!(selection_start(&app), 8);
    global(&mut app, GlobalCommand::SearchNext);
    assert_eq!(
        selection_start(&app),
        4,
        "the uppercase Dog never becomes a target"
    );
}

#[test]
fn last_query_survives_closing_and_reopening_the_panel() {
    let mut app = app_with("hi hi");
    open_find(&mut app);
    type_str(&mut app, "hi");
    press(&mut app, esc_key());

    open_find(&mut app);
    press(&mut app, esc_key());
    assert_eq!(
        app.last_find.as_ref().map(|(query, _)| query.as_str()),
        Some("hi")
    );

    global(&mut app, GlobalCommand::SearchNext);
    assert_eq!(selection_start(&app), 3);
}

#[test]
fn no_last_query_reports_feedback_instead_of_a_silent_no_op() {
    let mut app = app_with("hello");
    assert!(app.last_find.is_none());

    global(&mut app, GlobalCommand::SearchNext);

    assert_eq!(
        crate::messages::newest_text(&app),
        Some("no previous search"),
        "an unreachable chord must still give feedback, never swallow the keypress"
    );
}

#[test]
fn a_live_match_inside_a_concealed_links_url_reveals_the_whole_link() {
    let content = "prefix [text](hiddenword) suffix";
    let mut app = app_with(content);
    let link_open = content.find('[').expect("fixture has a link");

    open_find(&mut app);
    type_str(&mut app, "hiddenword");
    assert!(!find(&app).matches.is_empty());
    app.sync_view();

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = crate::render::build_rows(&app, crate::render::RowSource::Shown, view);
    assert!(
        rows.iter()
            .flatten()
            .any(|c| c.buf_offset == Some(link_open as u32)),
        "a live match inside the link's concealed url must reveal the whole link"
    );
}
