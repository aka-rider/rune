#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use crate::app::App;
use crate::keymap::{KeyCode, Mods};
use crate::pointer::{MouseInput, MouseKind};
use crate::runtime::{CmdKind, Effects, Msg, TimerKey, TimerMsgKey};

use super::tests::{CTRL, key, pump_index, seeded_app};

fn type_query(app: &mut App, text: &str, effects: &mut Effects) {
    for c in text.chars() {
        key(app, KeyCode::Char(c), Mods::NONE, effects);
    }
}

fn fire_debounce(app: &mut App, effects: &mut Effects) {
    crate::app::update(
        app,
        Msg::Timer {
            key: TimerMsgKey::ProjectSearchDebounce,
            generation: 0,
        },
        effects,
    );
}

fn query_cmd_count(effects: &Effects) -> usize {
    effects
        .cmds
        .iter()
        .filter(|cmd| cmd.kind() == CmdKind::ProjectQuery)
        .count()
}

fn run_one_query_cmd(effects: &mut Effects) -> Option<Msg> {
    let position = effects
        .cmds
        .iter()
        .position(|cmd| cmd.kind() == CmdKind::ProjectQuery)?;
    effects.cmds.remove(position).run()
}

fn search(app: &mut App, query: &str, effects: &mut Effects) {
    key(app, KeyCode::Char('F'), CTRL, effects);
    pump_index(app, effects);
    type_query(app, query, effects);
    fire_debounce(app, effects);
    let reply = run_one_query_cmd(effects).expect("the debounce dispatched a query cmd");
    crate::app::update(app, reply, effects);
}

fn result_displays(app: &App) -> Vec<String> {
    app.projectsearch()
        .expect("panel open")
        .results
        .iter()
        .map(|hit| hit.display.clone())
        .collect()
}

#[test]
fn typing_then_the_debounce_timer_dispatches_exactly_one_query_cmd() {
    let mut app = seeded_app(&[("/root/a.md", b"hello here")]);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    pump_index(&mut app, &mut effects);

    type_query(&mut app, "he", &mut effects);
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
fn a_one_char_query_dispatches_nothing() {
    let mut app = seeded_app(&[("/root/a.md", b"hello")]);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    pump_index(&mut app, &mut effects);

    type_query(&mut app, "h", &mut effects);
    fire_debounce(&mut app, &mut effects);

    assert_eq!(query_cmd_count(&effects), 0);
}

#[test]
fn the_reply_lists_files_ordered_by_match_count_descending() {
    let mut app = seeded_app(&[
        ("/root/one.md", b"needle"),
        ("/root/three.md", b"needle needle needle"),
        ("/root/two.md", b"needle and needle"),
    ]);
    let mut effects = Effects::default();

    search(&mut app, "needle", &mut effects);

    assert_eq!(result_displays(&app), vec!["three.md", "two.md", "one.md"]);
    let counts: Vec<usize> = app
        .projectsearch()
        .expect("panel open")
        .results
        .iter()
        .map(|hit| hit.count)
        .collect();
    assert_eq!(counts, vec![3, 2, 1]);
}

#[test]
fn an_uppercase_query_matches_case_sensitively() {
    let mut app = seeded_app(&[
        ("/root/lower.md", b"hello there"),
        ("/root/upper.md", b"Hello there"),
    ]);
    let mut effects = Effects::default();

    search(&mut app, "He", &mut effects);

    assert_eq!(result_displays(&app), vec!["upper.md"]);
}

#[test]
fn an_all_lowercase_query_matches_case_insensitively() {
    let mut app = seeded_app(&[
        ("/root/lower.md", b"hello there"),
        ("/root/upper.md", b"Hello there"),
    ]);
    let mut effects = Effects::default();

    search(&mut app, "he", &mut effects);

    assert_eq!(result_displays(&app), vec!["lower.md", "upper.md"]);
}

#[test]
fn a_multi_char_folded_scalar_still_matches_and_maps_back_to_text_offsets() {
    let city = "\u{130}stanbul is big".to_string();
    let mut app = seeded_app(&[("/root/turkish.md", city.as_bytes())]);
    let mut effects = Effects::default();

    search(&mut app, "i\u{307}st", &mut effects);

    let state = app.projectsearch().expect("panel open");
    let hit = state.results.first().expect("the folded form matches");
    assert_eq!(hit.display, "turkish.md");
    assert_eq!(hit.first_match, 0);
    assert_eq!(
        hit.ranges.first(),
        Some(&(0..4)),
        "the range spans the two-byte \u{130} plus \"st\" in the original text"
    );
    assert_eq!(hit.line, 1);
}

#[test]
fn a_stale_reply_is_dropped() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);
    let mut effects = Effects::default();
    search(&mut app, "needle", &mut effects);
    let live = app.projectsearch().expect("panel open").query_generation;

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
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    let scanned = super::tests::run_one_index_cmd(&mut effects).expect("scan cmd dispatched");
    crate::app::update(&mut app, scanned, &mut effects);
    assert!(
        app.project_index.as_ref().expect("index exists").building,
        "test setup: the read batches are still pending"
    );

    type_query(&mut app, "needle", &mut effects);
    fire_debounce(&mut app, &mut effects);
    let mid_build = run_one_query_cmd(&mut effects).expect("the debounce ran mid-build");
    crate::app::update(&mut app, mid_build, &mut effects);
    assert_eq!(
        result_displays(&app),
        Vec::<String>::new(),
        "the mid-build corpus had nothing yet"
    );

    pump_index(&mut app, &mut effects);

    let rerun = run_one_query_cmd(&mut effects)
        .expect("build completion reran the current query without a fresh debounce");
    crate::app::update(&mut app, rerun, &mut effects);
    assert_eq!(result_displays(&app), vec!["a.md"]);
}

#[test]
fn a_dirty_open_buffer_is_found_when_the_disk_copy_lacks_the_text() {
    let mut app = seeded_app(&[("/root/a.md", b"plain disk text")]);
    let mut effects = Effects::default();
    crate::workspace::open_path_checked(&mut app, Path::new("/root/a.md"), &mut effects)
        .expect("open the disk file");
    type_query(&mut app, "needle", &mut effects);
    assert!(
        app.active_doc().buffer.content().contains("needle"),
        "test setup: the buffer is dirty with text the disk lacks"
    );

    search(&mut app, "needle", &mut effects);

    assert_eq!(result_displays(&app), vec!["a.md"]);
    assert_eq!(
        app.projectsearch()
            .expect("panel open")
            .results
            .first()
            .map(|hit| hit.count),
        Some(1)
    );
}

#[test]
fn a_wheel_msg_over_the_panel_moves_the_selection() {
    let seeds: Vec<(String, Vec<u8>)> = (0..8)
        .map(|i| (format!("/root/f{i}.md"), b"needle".to_vec()))
        .collect();
    let refs: Vec<(&str, &[u8])> = seeds
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_slice()))
        .collect();
    let mut app = seeded_app(&refs);
    let mut effects = Effects::default();
    search(&mut app, "needle", &mut effects);
    assert_eq!(
        app.projectsearch().expect("panel open").list.cursor,
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
        app.projectsearch().expect("panel open").list.cursor,
        crate::commands::mouse::WHEEL_ROWS as usize
    );
}

#[test]
fn a_click_on_a_visible_row_opens_it_at_its_first_match() {
    let mut app = seeded_app(&[
        ("/root/a.md", b"needle"),
        ("/root/b.md", b"needle"),
        ("/root/c.md", b"needle"),
    ]);
    let mut effects = Effects::default();
    search(&mut app, "needle", &mut effects);

    let rect = crate::layout::geometry(app.frame_area(), &app).explorer_inner;
    crate::app::update(
        &mut app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(crate::pointer::MouseButton::Left),
            column: rect.x,
            row: rect.y + 3,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );

    assert!(
        app.projectsearch().is_none(),
        "a row click activates the result and closes the panel"
    );
    assert_eq!(
        app.active_doc().path(),
        Some(Path::new("/root/c.md")),
        "row 2 sits under the third list line"
    );
    assert_eq!(app.active_doc().cursors.primary().position.get(), 0);
    assert_eq!(app.focus(), crate::pane::Pane::Editor);
}

#[test]
fn erasing_below_the_minimum_clears_the_results_on_the_next_debounce() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);
    let mut effects = Effects::default();
    search(&mut app, "needle", &mut effects);
    assert_eq!(result_displays(&app), vec!["a.md"]);

    for _ in 0..5 {
        key(&mut app, KeyCode::Backspace, Mods::NONE, &mut effects);
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

    search(&mut app, "needle", &mut effects);

    assert_eq!(
        result_displays(&app),
        Vec::<String>::new(),
        "an open document outside the root neither overrides nor appears"
    );
}
