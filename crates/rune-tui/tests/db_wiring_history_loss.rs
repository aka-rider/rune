//! A stored undo history that cannot be used never costs the user their
//! text and never silently vanishes: the document still opens with its
//! recovered content, nothing is deleted from the store, and the user is
//! told. Driven through `rune_fuzz::Session`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;

use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::Effects;
use rune_tui::workspace;
use rune_vfs::VfsTestExt;

use db_wiring_common::harness::{DOC, Harness};

const BACKSPACE: KeyInput = KeyInput {
    code: KeyCode::Backspace,
    mods: Mods::NONE,
};

fn event_count(h: &Harness) -> i64 {
    let conn = rune_db::open_raw_connection_at_path_for_test(&h.db_path).expect("raw connection");
    conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .expect("count events")
}

fn tamper(h: &Harness, sql: &str) {
    let conn = rune_db::open_raw_connection_at_path_for_test(&h.db_path).expect("raw connection");
    let changed = conn.execute(sql, []).expect("tamper with the store");
    assert!(changed > 0, "the tampering matched nothing: {sql}");
}

fn log(h: &Harness) -> String {
    rune_tui::messages::log_text(h.session.app())
}

#[test]
fn a_history_that_does_not_replay_opens_with_its_text_keeps_every_row_and_warns() {
    let mut h = Harness::open("history-loss-unreplayable", b"hello");
    assert!(
        h.session
            .key(KeyInput {
                code: KeyCode::End,
                mods: Mods::NONE
            })
            .is_none()
    );
    assert!(h.session.key(BACKSPACE).is_none());
    h.settle();
    h.save();
    assert_eq!(h.content(), "hell");
    tamper(
        &h,
        "UPDATE events SET edits = replace(edits, '\"Deleted\":\"o\"', '\"Deleted\":\"X\"')",
    );
    let rows = event_count(&h);

    h.close_and_reopen();

    assert_eq!(h.content(), "hell");
    assert!(
        log(&h).contains("earlier undo history could not be replayed"),
        "{}",
        log(&h)
    );
    assert_eq!(event_count(&h), rows, "nothing is deleted from the store");
}

#[test]
fn a_history_with_an_unreadable_old_row_opens_with_its_text_and_warns() {
    let mut h = Harness::open("history-loss-unreadable", b"hello");
    h.type_at_end(" world");
    h.save();
    let doc_id = h.load_through_live_store().doc_id;
    let db = h.session.app().db.as_ref().expect("store");
    let op = db
        .store
        .create_snapshot(doc_id, "hello world")
        .expect("enqueue snapshot");
    rune_fuzz::driver::wait_for_db_op(&db.bridge, op);
    tamper(&h, "UPDATE events SET edits = '{'");
    let rows = event_count(&h);

    h.close_and_reopen();

    assert_eq!(h.content(), "hello world");
    assert!(
        log(&h).contains("earlier undo history could not be read"),
        "{}",
        log(&h)
    );
    assert_eq!(event_count(&h), rows, "nothing is deleted from the store");
}

#[test]
fn an_outside_change_that_drops_the_redo_tail_says_so() {
    let mut h = Harness::open("history-loss-redo-tail", b"hello");
    h.type_at_end(" world");
    h.save();
    h.undo();
    h.save();
    let doc_tab = h.session.app().active;
    workspace::open_path(h.session.app_mut(), Path::new("/other.md")).expect("open other");
    h.settle();
    let _ = workspace::close_now(h.session.app_mut(), doc_tab, &mut Effects::default());
    h.mem
        .save_atomic(Path::new(DOC), b"changed outside")
        .expect("outside change");

    workspace::open_path(h.session.app_mut(), Path::new(DOC)).expect("reopen doc");
    h.settle();

    assert_eq!(h.content(), "changed outside");
    assert!(
        log(&h).contains("earlier redo steps no longer apply"),
        "{}",
        log(&h)
    );
}

#[test]
fn an_outside_change_with_nothing_to_redo_raises_no_redo_notice() {
    let mut h = Harness::open("history-loss-no-redo-notice", b"hello");
    h.type_at_end(" world");
    h.save();
    let doc_tab = h.session.app().active;
    workspace::open_path(h.session.app_mut(), Path::new("/other.md")).expect("open other");
    h.settle();
    let _ = workspace::close_now(h.session.app_mut(), doc_tab, &mut Effects::default());
    h.mem
        .save_atomic(Path::new(DOC), b"changed outside")
        .expect("outside change");

    workspace::open_path(h.session.app_mut(), Path::new(DOC)).expect("reopen doc");
    h.settle();

    assert!(!log(&h).contains("redo steps"), "{}", log(&h));
}
