//! Repeat-counter coverage for the message-log pane, split out of
//! `messages.rs`: a message equal in severity and text to the newest entry
//! becomes one line with a counter, whichever severity posts it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod messages_common;

use rune_tui::app;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::messages;
use rune_tui::runtime::{Effects, Msg, TimerMsgKey};

use messages_common::{app_for, frame_text, key};

fn ctrl_z() -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char('z'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

fn lines_containing(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

#[test]
fn undoing_three_times_on_a_fresh_document_shows_one_nothing_to_undo_line_with_x3() {
    let mut session = app_for("hello");
    for _ in 0..3 {
        app::update(session.app_mut(), ctrl_z(), &mut Effects::default());
    }
    session.app_mut().sync_view();

    let text = frame_text(&mut session);
    assert_eq!(
        lines_containing(&text, "nothing to undo"),
        1,
        "repeats must collapse into one line, got {text:?}"
    );
    assert!(
        text.contains("nothing to undo x3"),
        "the repeat count must be shown, got {text:?}"
    );
}

#[test]
fn a_message_separated_from_its_twin_by_another_stays_three_lines_without_a_counter() {
    let mut session = app_for("hello");
    messages::info(session.app_mut(), "alpha");
    messages::info(session.app_mut(), "beta");
    messages::info(session.app_mut(), "alpha");
    session.app_mut().sync_view();

    assert_eq!(messages::log(session.app()).len(), 3);
    let text = frame_text(&mut session);
    assert_eq!(lines_containing(&text, "alpha"), 2);
    assert!(!text.contains(" x2"), "no entry repeated, got {text:?}");
}

#[test]
fn the_same_text_at_a_different_severity_is_a_separate_line() {
    let mut session = app_for("hello");
    messages::info(session.app_mut(), "disk full");
    messages::error(session.app_mut(), "disk full");
    session.app_mut().sync_view();

    assert_eq!(messages::log(session.app()).len(), 2);
    let text = frame_text(&mut session);
    assert_eq!(lines_containing(&text, "disk full"), 2);
    assert!(!text.contains(" x2"), "different severities, got {text:?}");
}

#[test]
fn a_single_post_shows_no_counter_suffix() {
    let mut session = app_for("hello");
    messages::info(session.app_mut(), "saved");
    session.app_mut().sync_view();

    let text = frame_text(&mut session);
    assert!(text.contains("saved"));
    assert!(!text.contains(" x1"), "a single post, got {text:?}");
}

#[test]
fn a_repeat_counts_as_a_post_and_reopens_a_collapsed_pane() {
    let mut session = app_for("hello");
    messages::info(session.app_mut(), "saved");
    app::update(
        session.app_mut(),
        key(KeyCode::Right),
        &mut Effects::default(),
    );
    app::update(
        session.app_mut(),
        Msg::Timer {
            key: TimerMsgKey::MessagesCollapse,
            generation: 0,
        },
        &mut Effects::default(),
    );
    assert!(
        !messages::is_open(session.app()),
        "test setup: the pane must have auto-collapsed"
    );
    let posts_before = messages::posts(session.app());

    messages::info(session.app_mut(), "saved");

    assert!(messages::is_open(session.app()));
    assert_eq!(messages::posts(session.app()), posts_before + 1);
    assert_eq!(messages::log(session.app()).len(), 1);
}

#[test]
fn a_text_with_a_control_character_counts_as_a_repeat_of_its_sanitized_twin() {
    let mut session = app_for("hello");
    messages::error(session.app_mut(), "bad\u{7}text");
    messages::error(session.app_mut(), "bad\u{7}text");
    session.app_mut().sync_view();

    assert_eq!(messages::log(session.app()).len(), 1);
    assert_eq!(messages::newest_text(session.app()), Some("badtext"));
    assert_eq!(messages::log(session.app())[0].count.get(), 2);
    assert_eq!(messages::posts(session.app()), 2);
    let text = frame_text(&mut session);
    assert!(text.contains("badtext x2"), "got {text:?}");
}
