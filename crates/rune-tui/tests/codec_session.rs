#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;

use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_vfs::Vfs;

use db_wiring_common::harness::{DOC, Harness};

const LATIN1: &[u8] = b"caf\xe9 au lait\n";

fn bare(code: KeyCode) -> KeyInput {
    KeyInput {
        code,
        mods: Mods::NONE,
    }
}

fn caret(h: &mut Harness) -> usize {
    h.session.snapshot().cursors[0].position.0
}

fn disk(h: &Harness) -> Vec<u8> {
    h.mem.read(Path::new(DOC)).unwrap()
}

#[test]
fn opening_a_latin1_file_warns_how_many_bytes_are_shown_as_raw() {
    let mut h = Harness::open("codec-warn", LATIN1);
    assert_eq!(h.content(), "caf\u{10FFE9} au lait\n");
    let warning = h.newest_message().expect("a warning on open");
    assert!(
        warning.contains("1 invalid UTF-8 byte(s) shown as <xx>"),
        "{warning:?}"
    );
}

#[test]
fn the_caret_steps_over_a_raw_byte_as_one_character() {
    let mut h = Harness::open("codec-caret", LATIN1);
    for _ in 0..3 {
        assert!(h.session.key(bare(KeyCode::Right)).is_none());
    }
    assert_eq!(caret(&mut h), 3);
    assert!(h.session.key(bare(KeyCode::Right)).is_none());
    assert_eq!(caret(&mut h), 3 + 4);
}

#[test]
fn an_edit_next_to_a_raw_byte_saves_with_only_the_typed_byte_added() {
    let mut h = Harness::open("codec-edit-save", LATIN1);
    for _ in 0..4 {
        assert!(h.session.key(bare(KeyCode::Right)).is_none());
    }
    assert!(h.session.type_("X").is_none());
    h.settle();
    h.save();
    assert_eq!(disk(&h), b"caf\xe9X au lait\n");
}

#[test]
fn a_save_without_edits_leaves_the_original_bytes() {
    let mut h = Harness::open("codec-clean-save", LATIN1);
    h.save();
    assert_eq!(disk(&h), LATIN1);
}

#[test]
fn undoing_back_to_the_original_text_and_saving_restores_the_original_bytes() {
    let mut h = Harness::open("codec-undo-save", LATIN1);
    assert!(h.session.type_("X").is_none());
    h.settle();
    h.save();
    assert_ne!(disk(&h), LATIN1);
    h.undo();
    h.save();
    assert_eq!(disk(&h), LATIN1);
}
