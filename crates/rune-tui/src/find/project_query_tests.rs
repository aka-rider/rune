use std::path::Path;

use super::test_support::*;
use crate::keymap::KeyCode;
use crate::pointer::{MouseButton, MouseInput, MouseKind};
use crate::runtime::{CmdKind, Effects, Msg, TimerKey, TimerMsgKey};

fn query_cmd_count(effects: &Effects) -> usize {
    effects
        .cmds
        .iter()
        .filter(|cmd| cmd.kind() == CmdKind::ProjectQuery)
        .count()
}

#[test]
fn typing_then_the_debounce_timer_dispatches_exactly_one_query_cmd() {
    let mut app = seeded_app(&[("/root/a.md", b"hello here")]);
    let mut effects = open_project_find(&mut app);
    pump_index(&mut app, &mut effects);

    type_into(&mut app, "he", &mut effects);
    assert_eq!(
        query_cmd_count(&effects),
        0,
        "keystrokes only arm the timer, never dispatch"
    );
    assert!(
        app.timers
            .armed_deadline(TimerKey::from(TimerMsgKey::ProjectSearchDebounce))
            .is_some(),
        "an edit arms the debounce timer"
    );

    fire_debounce(&mut app, &mut effects);

    assert_eq!(query_cmd_count(&effects), 1);
}

#[test]
fn a_one_char_query_dispatches_nothing_and_the_readout_asks_for_more() {
    let mut app = seeded_app(&[("/root/a.md", b"hello")]);
    let mut effects = open_project_find(&mut app);
    pump_index(&mut app, &mut effects);

    type_into(&mut app, "h", &mut effects);
    fire_debounce(&mut app, &mut effects);

    assert_eq!(query_cmd_count(&effects), 0);
    assert!(
        project_readout_row(&mut app).contains("2+ chars"),
        "{:?}",
        project_readout_row(&mut app)
    );
}

#[test]
fn the_reply_lists_files_ordered_by_match_count_descending() {
    let mut app = seeded_app(&[
        ("/root/one.md", b"needle"),
        ("/root/three.md", b"needle needle needle"),
        ("/root/two.md", b"needle and needle"),
    ]);
    let mut effects = Effects::default();

    search_project(&mut app, "needle", &mut effects);

    assert_eq!(result_displays(&app), vec!["three.md", "two.md", "one.md"]);
    let counts: Vec<usize> = project(&app).results.iter().map(|hit| hit.count).collect();
    assert_eq!(counts, vec![3, 2, 1]);
}

#[test]
fn the_case_toggle_governs_project_matching() {
    let mut app = seeded_app(&[
        ("/root/lower.md", b"hello there"),
        ("/root/upper.md", b"Hello there"),
    ]);
    let mut effects = Effects::default();
    search_project(&mut app, "He", &mut effects);
    assert_eq!(
        result_displays(&app),
        vec!["lower.md", "upper.md"],
        "case-insensitive by default, an uppercase query is no longer smart-case"
    );

    press_into(&mut app, key(KeyCode::Char('c'), ALT), &mut effects);
    assert!(find(&app).options.case_sensitive);
    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);

    assert_eq!(result_displays(&app), vec!["upper.md"]);
}

#[test]
fn the_word_and_regex_toggles_govern_project_matching() {
    let mut app = seeded_app(&[
        ("/root/whole.md", b"a cat sat cot"),
        ("/root/partial.md", b"category"),
    ]);
    let mut effects = Effects::default();
    search_project(&mut app, "cat", &mut effects);
    assert_eq!(result_displays(&app), vec!["partial.md", "whole.md"]);

    press_into(&mut app, key(KeyCode::Char('w'), ALT), &mut effects);
    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);
    assert_eq!(result_displays(&app), vec!["whole.md"]);

    press_into(&mut app, key(KeyCode::Char('w'), ALT), &mut effects);
    press_into(&mut app, key(KeyCode::Char('r'), ALT), &mut effects);
    press_into(&mut app, backspace(), &mut effects);
    press_into(&mut app, backspace(), &mut effects);
    type_into(&mut app, ".t", &mut effects);
    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);
    assert_eq!(result_displays(&app), vec!["whole.md", "partial.md"]);
    assert_eq!(project(&app).match_total(), 3);
}

#[test]
fn an_invalid_regex_clears_the_results_and_shows_the_error() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    assert_eq!(result_displays(&app), vec!["a.md"]);

    press_into(&mut app, key(KeyCode::Char('r'), ALT), &mut effects);
    type_into(&mut app, "(", &mut effects);
    fire_debounce(&mut app, &mut effects);

    assert_eq!(query_cmd_count(&effects), 0);
    assert!(result_displays(&app).is_empty());
    assert!(project_readout_row(&mut app).contains("regex parse error"));
}

#[test]
fn a_stale_reply_is_dropped() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    let live = project(&app).query_generation;

    let stale = crate::generation::ProjectSearchGen::from_raw(live.raw() + 1);
    crate::app::update(
        &mut app,
        Msg::ProjectSearchQueried {
            generation: stale,
            results: Vec::new(),
            truncated: false,
        },
        &mut effects,
    );

    assert_eq!(
        result_displays(&app),
        vec!["a.md"],
        "a stale reply must never replace live results"
    );
}

#[test]
fn a_query_answered_mid_build_reruns_after_the_final_batch() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);
    let mut effects = open_project_find(&mut app);
    let scanned = run_one_cmd(&mut effects, CmdKind::ProjectIndex).expect("scan cmd dispatched");
    crate::app::update(&mut app, scanned, &mut effects);
    assert!(
        app.project_index.as_ref().expect("index exists").building,
        "test setup: the read batches are still pending"
    );

    type_into(&mut app, "needle", &mut effects);
    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);
    assert_eq!(
        result_displays(&app),
        Vec::<String>::new(),
        "the mid-build corpus had nothing yet"
    );

    pump_index(&mut app, &mut effects);

    deliver_query(&mut app, &mut effects);
    assert_eq!(result_displays(&app), vec!["a.md"]);
}

#[test]
fn a_dirty_open_buffer_is_found_when_the_disk_copy_lacks_the_text() {
    let mut app = seeded_app(&[("/root/a.md", b"plain disk text")]);
    let mut effects = Effects::default();
    crate::workspace::open_path_checked(&mut app, Path::new("/root/a.md"), &mut effects)
        .expect("open the disk file");
    type_into(&mut app, "needle", &mut effects);
    assert!(
        app.active_doc().buffer.content().contains("needle"),
        "test setup: the buffer is dirty with text the disk lacks"
    );

    search_project(&mut app, "needle", &mut effects);

    assert_eq!(result_displays(&app), vec!["a.md"]);
    assert_eq!(project(&app).results.first().map(|hit| hit.count), Some(1));
}

fn eight_files() -> Vec<(String, Vec<u8>)> {
    (0..8)
        .map(|i| (format!("/root/f{i}.md"), b"needle".to_vec()))
        .collect()
}

#[test]
fn a_wheel_msg_over_the_results_moves_the_selection() {
    let seeds = eight_files();
    let refs: Vec<(&str, &[u8])> = seeds
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_slice()))
        .collect();
    let mut app = seeded_app(&refs);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    assert_eq!(
        project(&app).list.cursor,
        0,
        "test setup: fresh results select the first row"
    );

    let rect = crate::layout::geometry(app.frame_area(), &app).explorer_inner;
    crate::app::update(
        &mut app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::ScrollDown,
            column: rect.x,
            row: rect.y + 2,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );

    assert_eq!(
        project(&app).list.cursor,
        crate::commands::mouse::WHEEL_ROWS as usize
    );
}

#[test]
fn a_click_on_a_visible_row_opens_it_and_keeps_the_panel_focused_on_the_results() {
    let mut app = seeded_app(&[
        ("/root/a.md", b"needle"),
        ("/root/b.md", b"needle"),
        ("/root/c.md", b"needle"),
    ]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);

    let rect = crate::layout::geometry(app.frame_area(), &app).explorer_inner;
    crate::app::update(
        &mut app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y + 2,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );

    assert!(find(&app).focused, "a row click keeps the panel focused");
    assert_eq!(find(&app).focus, crate::find::Control::Results);
    assert_eq!(
        app.active_doc().path(),
        Some(Path::new("/root/c.md")),
        "the third list row is the third hit; results start at the column's first row"
    );
    assert_eq!(selection_start(&app), 0);
}

#[test]
fn erasing_below_the_minimum_clears_the_results_on_the_next_debounce() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    assert_eq!(result_displays(&app), vec!["a.md"]);

    for _ in 0..5 {
        press_into(&mut app, backspace(), &mut effects);
    }
    fire_debounce(&mut app, &mut effects);

    assert_eq!(query_cmd_count(&effects), 0);
    assert_eq!(result_displays(&app), Vec::<String>::new());
}

#[test]
fn a_document_outside_the_root_never_overrides() {
    let mut app = seeded_app(&[
        ("/root/a.md", b"plain"),
        ("/elsewhere/far.md", b"needle far away"),
    ]);
    let mut effects = Effects::default();
    crate::workspace::open_path_checked(&mut app, Path::new("/elsewhere/far.md"), &mut effects)
        .expect("open the outside file");

    search_project(&mut app, "needle", &mut effects);

    assert_eq!(
        result_displays(&app),
        Vec::<String>::new(),
        "an open document outside the root neither overrides nor appears"
    );
}

#[test]
fn a_reply_never_reminted_by_typing_keeps_its_generation_until_the_debounce_fires() {
    let mut app = seeded_app(&[("/root/a.md", b"hi")]);
    let mut effects = open_project_find(&mut app);
    let minted = project(&app).query_generation;

    type_into(&mut app, "hi", &mut effects);
    press_into(&mut app, backspace(), &mut effects);

    assert_eq!(find(&app).find.editor.text(), "h");
    assert_eq!(project(&app).query_generation, minted);
    assert_eq!(
        app.active_doc().buffer.content(),
        "hello",
        "typing into the panel must never reach the editor"
    );
}
