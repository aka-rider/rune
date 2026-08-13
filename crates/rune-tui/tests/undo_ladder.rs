#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;
use std::time::Duration;

use rune_core::buffer::{AppliedEdit, Buffer};
use rune_core::cursor::CursorSet;
use rune_core::undo::{EditKind, Step};
use rune_tui::app::{self, App};
use rune_tui::commands::edit;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::merge::MergeState;
use rune_tui::pointer::ManualClock;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

fn app_with_clock(content: &str) -> (App, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new());
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.clock = clock.clone();
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(content.len());
    app.doc_mut(id).unwrap().viewport.set_size(80, 23);
    (app, clock)
}

fn send(app: &mut App, msg: Msg) {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
}

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        send(
            app,
            Msg::Key(KeyInput {
                code: KeyCode::Char(c),
                mods: Mods::NONE,
            }),
        );
    }
}

fn press_enter(app: &mut App) {
    send(
        app,
        Msg::Key(KeyInput {
            code: KeyCode::Enter,
            mods: Mods::NONE,
        }),
    );
}

fn press_undo(app: &mut App) {
    send(
        app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('z'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        }),
    );
}

fn press_redo(app: &mut App) {
    send(
        app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('y'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        }),
    );
}

fn content(app: &App) -> String {
    app.active_doc().buffer.content().to_string()
}

#[test]
fn one_press_after_typing_removes_one_rune() {
    let (mut app, _clock) = app_with_clock("");
    type_str(&mut app, "ab");
    press_undo(&mut app);
    assert_eq!(content(&app), "a");
}

#[test]
fn a_second_press_reaches_a_word_boundary() {
    let (mut app, _clock) = app_with_clock("");
    type_str(&mut app, "hi there");
    press_undo(&mut app);
    assert_eq!(content(&app), "hi ther");
    press_undo(&mut app);
    assert_eq!(
        content(&app),
        "hi",
        "the second press must climb to Word and remove the trailing \
         partial word plus the whitespace before it"
    );
}

#[test]
fn the_fifth_and_later_presses_each_remove_one_line() {
    let (mut app, _clock) = app_with_clock("");
    for i in 0..7 {
        type_str(&mut app, "xx");
        if i < 6 {
            press_enter(&mut app);
        }
    }
    assert_eq!(
        content(&app),
        "xx\nxx\nxx\nxx\nxx\nxx\nxx",
        "fixture must be seven lines of \"xx\""
    );

    for _ in 0..4 {
        press_undo(&mut app);
    }
    assert_eq!(
        content(&app),
        "xx\nxx",
        "presses one through four (Rune/Word/MultiWord/Sentence) must have \
         reduced the buffer to exactly two lines"
    );

    press_undo(&mut app);
    assert_eq!(
        content(&app),
        "xx",
        "the fifth press (Line tier) must remove exactly one whole line"
    );

    press_undo(&mut app);
    assert_eq!(
        content(&app),
        "",
        "the sixth press (still Line tier) must remove exactly one more line"
    );
}

#[test]
fn advancing_the_clock_past_ladder_reset_restarts_the_ladder_at_rune() {
    let (mut app, clock) = app_with_clock("");
    type_str(&mut app, "hi there");

    press_undo(&mut app);
    assert_eq!(content(&app), "hi ther");

    clock.advance(Duration::from_millis(600));

    press_undo(&mut app);
    assert_eq!(
        content(&app),
        "hi the",
        "a press after the pause window must restart at Rune (one rune \
         removed), not continue on to Word"
    );
}

#[test]
fn n_undos_then_n_redos_restore_the_buffer_byte_for_byte() {
    let (mut app, _clock) = app_with_clock("");
    type_str(&mut app, "hi there");
    let original = content(&app);

    press_undo(&mut app);
    press_undo(&mut app);
    assert_eq!(content(&app), "hi");

    press_redo(&mut app);
    press_redo(&mut app);
    assert_eq!(content(&app), original);
}

#[test]
fn a_paste_step_is_one_unit_at_every_tier() {
    let (mut app, _clock) = app_with_clock("");
    let id = app.active;
    send(&mut app, Msg::Paste("AAA".to_string()));
    send(&mut app, Msg::Paste("BBB".to_string()));
    send(&mut app, Msg::Paste("CCC".to_string()));
    assert_eq!(content(&app), "AAABBBCCC");
    assert_eq!(app.doc(id).unwrap().journal.len(), 3);

    press_undo(&mut app);
    assert_eq!(content(&app), "AAABBB");

    press_undo(&mut app);
    assert_eq!(
        content(&app),
        "AAA",
        "the second press must still undo exactly one paste, never absorbing \
         a neighbour even though the ladder has grown"
    );

    press_undo(&mut app);
    assert_eq!(content(&app), "");
}

#[test]
fn merge_active_consumes_exactly_one_step_per_press() {
    let (mut app, _clock) = app_with_clock("");
    type_str(&mut app, "hello world");
    let id = app.active;
    assert_eq!(app.doc(id).unwrap().journal.pos(), 11);

    app.merge = MergeState::Active {
        doc: id,
        pairs: Vec::new(),
        cur: 0,
        saved_display_name: None,
        theirs_obs: rune_db::ObsId::new(1).expect("nonzero"),
    };

    edit::undo(&mut app, id);
    assert_eq!(
        app.doc(id).unwrap().journal.pos(),
        10,
        "with merge active a press must consume exactly one step, never a \
         growing ladder group"
    );

    edit::undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().journal.pos(), 9);
}

#[test]
fn a_mid_run_apply_failure_leaves_the_durable_position_at_the_step_actually_reached() {
    let (mut app, _clock) = app_with_clock("abZ9");
    let id = app.active;

    let cursors = CursorSet::new(4).all().to_vec();
    {
        let doc = app.doc_mut(id).unwrap();
        doc.journal.push(Step {
            edits: vec![AppliedEdit {
                start: 100,
                end: 100,
                deleted: String::new(),
                insert: "Q".to_string(),
            }],
            cursors_before: cursors.clone(),
            cursors_after: cursors.clone(),
            kind: EditKind::Insert,
        });
        doc.journal.push(Step {
            edits: vec![AppliedEdit {
                start: 2,
                end: 3,
                deleted: String::new(),
                insert: "Z".to_string(),
            }],
            cursors_before: cursors.clone(),
            cursors_after: cursors.clone(),
            kind: EditKind::Insert,
        });
        doc.journal.push(Step {
            edits: vec![AppliedEdit {
                start: 3,
                end: 4,
                deleted: String::new(),
                insert: "9".to_string(),
            }],
            cursors_before: cursors.clone(),
            cursors_after: cursors.clone(),
            kind: EditKind::Insert,
        });
    }
    assert_eq!(app.doc(id).unwrap().journal.pos(), 3);

    press_undo(&mut app);
    assert_eq!(content(&app), "abZ");
    assert_eq!(app.doc(id).unwrap().journal.pos(), 2);

    press_undo(&mut app);
    assert_eq!(
        content(&app),
        "ab",
        "the successful first step of this press must still have landed"
    );
    assert_eq!(
        app.doc(id).unwrap().journal.pos(),
        1,
        "the journal position (mirrored to the durable store) must sit at \
         the step actually reached, not the full planned Word-tier count \
         and not the pre-press position"
    );
    assert!(
        rune_tui::messages::newest_text(&app)
            .unwrap_or_default()
            .starts_with("undo failed"),
        "the failed second step must surface an error"
    );
}

#[test]
fn flipping_direction_restarts_the_ladder_at_rune() {
    let (mut app, _clock) = app_with_clock("");
    let id = app.active;
    type_str(&mut app, "alpha beta gamma delta");

    for _ in 0..4 {
        press_undo(&mut app);
    }
    let after_undo_run = app.doc(id).unwrap().journal.pos();

    press_redo(&mut app);
    assert_eq!(
        app.doc(id).unwrap().journal.pos(),
        after_undo_run + 1,
        "the first press of a redo run must restart the ladder at Rune \
         rather than continue the undo run's tier, which would overshoot \
         the position the run started from"
    );
}

#[test]
fn a_redo_run_stops_at_the_position_its_undo_run_started_from() {
    let (mut app, clock) = app_with_clock("");
    let id = app.active;
    type_str(&mut app, "alpha beta gamma delta");

    press_undo(&mut app);
    clock.advance(Duration::from_millis(1_000));
    let run_start = app.doc(id).unwrap().journal.pos();

    for _ in 0..8 {
        press_undo(&mut app);
    }
    assert_eq!(app.doc(id).unwrap().journal.pos(), 0);

    for _ in 0..8 {
        press_redo(&mut app);
        if app.doc(id).unwrap().journal.pos() >= run_start {
            break;
        }
    }
    assert_eq!(
        app.doc(id).unwrap().journal.pos(),
        run_start,
        "a redo run must land exactly where its undo run began rather than \
         overshoot and resurrect an edit undone before the run started"
    );

    press_redo(&mut app);
    assert!(
        app.doc(id).unwrap().journal.pos() > run_start,
        "once the barrier is reached a further press may travel past it"
    );
}
