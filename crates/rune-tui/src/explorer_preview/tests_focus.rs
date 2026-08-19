#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use rune_vfs::VfsTestExt;
use std::sync::Arc;

use rune_core::buffer::Buffer as CoreBuffer;
use rune_vfs::{Mem, Vfs};

use super::*;
use super::tests_common::{app_with, load_entries, run_cmds};
use crate::runtime::Msg;

#[test]
fn ctrl_2_while_previewing_discards_and_restores_the_original_tab_count() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/c.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    // Two real tabs BEFORE the preview mints a third, so `^2` (`TabSwitch(1)`,
    // the SECOND tab) targets the pre-existing real one, not the preview.
    app.open_document(CoreBuffer::new("second"));
    let tabs_before = app.documents.order().len();
    load_entries(&mut app, &["c.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let preview_id = app.explorer.preview.expect("preview minted");
    assert_eq!(app.documents.order().len(), tabs_before + 1);

    let ctrl_2 = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char('2'),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    };
    crate::app::update(&mut app, Msg::Key(ctrl_2), &mut effects);

    assert_eq!(app.documents.order().len(), tabs_before);
    assert!(app.explorer.preview.is_none());
    assert!(app.doc(preview_id).is_none(), "removed from app.documents");
}

/// `^e` (`GlobalCommand::FocusEditor`) no longer exists in the shipped
/// keymap — the keys half of this merge deleted it. Escape from the
/// Explorer (`ExplorerCommand::Leave`) is chosen as its replacement route
/// over `^B`'s hide branch: both are pure focus transitions that reach
/// `on_focus_changed` with no document switch of their own, but Escape is
/// the one a user actually presses right after arrowing the Explorer (the
/// same gesture that minted the preview in the first place), so it
/// exercises the promote hook against the exact sequence the feature is
/// for. `^B`'s hide branch gets its own dedicated coverage below.
#[test]
fn escape_from_the_explorer_promotes_the_live_preview() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let id = app.explorer.preview.expect("preview minted");
    let tabs_before = app.documents.order().len();
    // `on_focus_changed` only reacts to an actual TRANSITION — land on the
    // Explorer first (browsing it is what minted the preview above in the
    // first place).
    app.set_focus_pane(Pane::Explorer, &mut effects);

    let escape = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Escape,
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(&mut app, Msg::Key(escape), &mut effects);

    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.doc(id).unwrap().read_only, ReadOnly::No);
    assert!(app.explorer.preview.is_none(), "preview slot cleared");
    assert_eq!(
        app.documents.order().iter().filter(|&&t| t == id).count(),
        1,
        "promoted document appears exactly once in documents.order()"
    );
    assert_eq!(
        app.documents.order().len(),
        tabs_before,
        "promotion mints no extra tab"
    );
}

/// `^B` (`GlobalCommand::ToggleLeft`)'s hide branch: painted this frame ⇒
/// hides the column and hands focus to the Editor. That's the second, and
/// last, pure-focus route into the Editor the shipped keymap has —
/// `pane::handle_global_command`'s `ToggleLeft` arm calls `set_focus_pane`
/// directly, touching no document, exactly like Escape above.
#[test]
fn ctrl_b_hiding_the_column_promotes_the_live_preview() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let id = app.explorer.preview.expect("preview minted");
    app.set_focus_pane(Pane::Explorer, &mut effects);
    assert!(app.splits.left.is_shown(), "column starts visible");

    let ctrl_b = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char('b'),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    };
    crate::app::update(&mut app, Msg::Key(ctrl_b), &mut effects);

    assert!(!app.splits.left.is_shown(), "the column collapses");
    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.doc(id).unwrap().read_only, ReadOnly::No);
    assert!(app.explorer.preview.is_none());
}

/// Enter on a file row promotes through `explorer_keys::open_selected`'s own
/// direct call to `explorer_preview::promote` — not through
/// `on_focus_changed`'s focus-transition hook, though that hook also fires
/// (harmlessly, as a no-op — the preview slot is already clear by the time
/// it runs) since `Open` moves focus to the Editor too. Driven through a
/// real `Enter` key message rather than calling `promote` directly, so this
/// proves the whole route the keymap actually offers, not just the
/// function in isolation.
#[test]
fn enter_on_a_file_row_promotes_the_preview_via_the_direct_call_path() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let id = app.explorer.preview.expect("preview minted");
    let tabs_before = app.documents.order().len();
    app.set_focus_pane(Pane::Explorer, &mut effects);

    let enter = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Enter,
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(&mut app, Msg::Key(enter), &mut effects);

    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.doc(id).unwrap().read_only, ReadOnly::No);
    assert!(app.explorer.preview.is_none());
    assert_eq!(
        app.documents.order().iter().filter(|&&t| t == id).count(),
        1,
        "exactly one tab for the promoted document"
    );
    assert_eq!(
        app.documents.order().len(),
        tabs_before,
        "no second tab appears"
    );
}

/// Escape from the Tabs pane also lands on the Editor — the same
/// `on_focus_changed` `Pane::Editor` arm Escape-from-Explorer and `^B`'s
/// hide branch reach. The difference: reaching the Tabs pane at all
/// (`^t`, `GlobalCommand::FocusTabs`) is itself a focus transition that
/// `on_focus_changed`'s `Pane::Title | Pane::Tabs` arm discards the live
/// preview for, in the SAME `app::update` call that moved focus there — so
/// by the time a later, separate Escape keypress reaches the Tabs pane, no
/// preview is left to promote. This pins that the code does NOT double-fire
/// a promote against an already-discarded preview, and leaves no dangling
/// document behind either.
#[test]
fn escape_from_the_tabs_pane_has_no_preview_left_to_promote() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    let real = app.active;
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let preview_id = app.explorer.preview.expect("preview minted");
    app.set_focus_pane(Pane::Explorer, &mut effects);

    let ctrl_t = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char('t'),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    };
    crate::app::update(&mut app, Msg::Key(ctrl_t), &mut effects);
    assert!(
        app.explorer.preview.is_none(),
        "landing on Tabs already discarded it"
    );
    assert!(app.doc(preview_id).is_none());

    let escape = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Escape,
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(&mut app, Msg::Key(escape), &mut effects);

    assert_eq!(app.focus(), Pane::Editor);
    assert!(app.explorer.preview.is_none());
    assert!(
        !app.documents.order().contains(&preview_id),
        "the discarded preview never reappears"
    );
    assert_eq!(app.active, real, "the surviving real tab stays active");
}
