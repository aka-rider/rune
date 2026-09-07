use rune_fuzz::invariant::find_preview_pure;
use rune_fuzz::snapshot::Snapshot;
use rune_fuzz::step::{MsgTag, StepCtx};
use rune_tui::keymap::{KeyCode, Mods};

use crate::support::{base_active_id, base_ctx, base_snapshot, key};

const ALT: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: false,
};

fn key_ctx(code: KeyCode, mods: Mods) -> StepCtx {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(code, mods),
        command: None,
    };
    ctx
}

fn replace_focused(content: &str, version: u64) -> Snapshot {
    let mut snap = base_snapshot(content);
    snap.replace_field_focused = true;
    snap.version_by_doc.insert(base_active_id(), version);
    snap
}

#[test]
fn find_preview_pure_detects_an_edit_while_typing_into_the_replace_field() {
    let prev = replace_focused("dog", 1);
    let next = replace_focused("cat", 2);
    let v = find_preview_pure(&prev, &next, &key_ctx(KeyCode::Char('c'), Mods::NONE))
        .expect("a typed char in the Replace field bumped a buffer version");
    assert_eq!(v.id, "FIND-PREVIEW-PURE");
}

#[test]
fn find_preview_pure_accepts_typing_that_leaves_every_version_alone() {
    let prev = replace_focused("dog", 1);
    let next = replace_focused("dog", 1);
    assert_eq!(
        find_preview_pure(&prev, &next, &key_ctx(KeyCode::Char('c'), Mods::NONE)),
        None
    );
}

#[test]
fn find_preview_pure_ignores_typing_into_the_find_field() {
    let mut prev = replace_focused("dog", 1);
    prev.replace_field_focused = false;
    let mut next = replace_focused("cat", 2);
    next.replace_field_focused = false;
    assert_eq!(
        find_preview_pure(&prev, &next, &key_ctx(KeyCode::Char('c'), Mods::NONE)),
        None
    );
}

#[test]
fn find_preview_pure_ignores_chords_and_non_character_keys() {
    let prev = replace_focused("dog", 1);
    let next = replace_focused("cat", 2);
    assert_eq!(
        find_preview_pure(&prev, &next, &key_ctx(KeyCode::Char('c'), ALT)),
        None,
        "an option toggle is not typing"
    );
    assert_eq!(
        find_preview_pure(&prev, &next, &key_ctx(KeyCode::Enter, Mods::NONE)),
        None,
        "Enter in the Replace field is the replace command itself"
    );
}

#[test]
fn find_preview_pure_ignores_a_document_that_only_exists_after_the_step() {
    let prev = replace_focused("dog", 1);
    let mut next = replace_focused("dog", 1);
    next.version_by_doc
        .insert(crate::support::other_doc_id(), 7);
    assert_eq!(
        find_preview_pure(&prev, &next, &key_ctx(KeyCode::Char('c'), Mods::NONE)),
        None
    );
}
