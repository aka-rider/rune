use rune_fuzz::invariant::{
    find_esc_closes, find_preview_pure, find_replace_no_disk, find_walk_drains,
};
use rune_fuzz::snapshot::Snapshot;
use rune_fuzz::step::{MsgTag, StepCtx};
use rune_tui::focus::FocusTarget;
use rune_tui::keymap::{KeyCode, Mods};

use crate::support::{base_active_id, base_ctx, base_snapshot, key};

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

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

fn replace_all_ctx(disk_before: Option<&[u8]>, disk: Option<&[u8]>) -> StepCtx {
    let mut ctx = key_ctx(KeyCode::Enter, SHIFT);
    ctx.disk_before = disk_before.map(<[u8]>::to_vec);
    ctx.disk = disk.map(<[u8]>::to_vec);
    ctx
}

fn panel_focused(content: &str) -> Snapshot {
    let mut snap = base_snapshot(content);
    snap.focus_target = FocusTarget::Find;
    snap
}

#[test]
fn find_replace_no_disk_detects_a_disk_change_on_replace_all() {
    let prev = panel_focused("dog");
    let v = find_replace_no_disk(&prev, &replace_all_ctx(Some(b"dog"), Some(b"cat")))
        .expect("replace-all rewrote the file on disk");
    assert_eq!(v.id, "FIND-REPLACE-NO-DISK");
}

#[test]
fn find_replace_no_disk_accepts_an_unchanged_disk() {
    let prev = panel_focused("dog");
    assert_eq!(
        find_replace_no_disk(&prev, &replace_all_ctx(Some(b"dog"), Some(b"dog"))),
        None
    );
    assert_eq!(
        find_replace_no_disk(&prev, &replace_all_ctx(None, None)),
        None,
        "a never-saved file stays never-saved"
    );
}

#[test]
fn find_replace_no_disk_ignores_steps_outside_the_panel() {
    let mut prev = panel_focused("dog");
    prev.focus_target = FocusTarget::Editor;
    assert_eq!(
        find_replace_no_disk(&prev, &replace_all_ctx(Some(b"dog"), Some(b"cat"))),
        None,
        "a save chord in the editor is allowed to change the disk"
    );
    let prev = panel_focused("dog");
    let mut ctx = replace_all_ctx(Some(b"dog"), Some(b"cat"));
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Enter, Mods::NONE),
        command: None,
    };
    assert_eq!(
        find_replace_no_disk(&prev, &ctx),
        None,
        "only the replace-all chord is checked"
    );
}

#[test]
fn find_walk_drains_detects_an_idle_walk_that_survived_the_step() {
    let mut next = base_snapshot("dog");
    next.find_walk_queued = Some(0);
    let v = find_walk_drains(&next).expect("an empty queue must finish the walk");
    assert_eq!(v.id, "FIND-WALK-DRAINS");
}

#[test]
fn find_walk_drains_accepts_a_waiting_walk_and_no_walk() {
    let mut next = base_snapshot("dog");
    assert_eq!(find_walk_drains(&next), None);
    next.find_walk_queued = Some(2);
    assert_eq!(find_walk_drains(&next), None);
}

fn panel_snapshot(open: bool) -> Snapshot {
    let mut snap = base_snapshot("dog");
    snap.find_open = open;
    snap
}

#[test]
fn find_esc_closes_detects_a_panel_that_survived_a_plain_escape() {
    let ctx = key_ctx(KeyCode::Escape, Mods::NONE);
    let v = find_esc_closes(&panel_snapshot(true), &panel_snapshot(true), &ctx)
        .expect("Esc must close the panel whatever had focus");
    assert_eq!(v.id, "FIND-ESC-CLOSES");
}

#[test]
fn find_esc_closes_accepts_a_closed_panel_a_modified_escape_and_other_keys() {
    let open = panel_snapshot(true);
    let closed = panel_snapshot(false);
    let esc = key_ctx(KeyCode::Escape, Mods::NONE);
    assert_eq!(find_esc_closes(&open, &closed, &esc), None);
    assert_eq!(find_esc_closes(&closed, &closed, &esc), None);
    assert_eq!(
        find_esc_closes(&open, &open, &key_ctx(KeyCode::Escape, SHIFT)),
        None
    );
    assert_eq!(
        find_esc_closes(&open, &open, &key_ctx(KeyCode::Char('x'), Mods::NONE)),
        None
    );
}

#[test]
fn find_esc_closes_yields_to_a_modal_that_owns_the_key() {
    let mut prev = panel_snapshot(true);
    prev.modal_open = true;
    let ctx = key_ctx(KeyCode::Escape, Mods::NONE);
    assert_eq!(find_esc_closes(&prev, &panel_snapshot(true), &ctx), None);
}

#[test]
fn find_esc_closes_yields_to_a_pane_whose_own_escape_returns_focus_first() {
    let ctx = key_ctx(KeyCode::Escape, Mods::NONE);
    for target in [
        FocusTarget::Messages,
        FocusTarget::Title,
        FocusTarget::Explorer,
        FocusTarget::Tabs,
    ] {
        let mut prev = panel_snapshot(true);
        prev.focus_target = target;
        assert_eq!(
            find_esc_closes(&prev, &panel_snapshot(true), &ctx),
            None,
            "{target:?}"
        );
    }
}
