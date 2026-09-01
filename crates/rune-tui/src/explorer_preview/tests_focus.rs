#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use rune_vfs::VfsTestExt;
use std::sync::Arc;

use rune_core::buffer::Buffer as CoreBuffer;
use rune_vfs::Mem;

use super::tests_common::{app_with, load_entries, run_cmds};
use super::*;
use crate::document::ReadOnly;
use crate::runtime::Msg;

fn ctrl(c: char) -> crate::keymap::KeyInput {
    crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char(c),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    }
}

const ESCAPE: crate::keymap::KeyInput = crate::keymap::KeyInput {
    code: crate::keymap::KeyCode::Escape,
    mods: crate::keymap::Mods::NONE,
};

fn preview_one_file(app: &mut App, effects: &mut Effects) {
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(app, effects);
    run_cmds(app, effects);
}

#[test]
fn ctrl_2_while_previewing_discards_without_touching_the_tab_strip() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/c.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    let second = app.open_document(CoreBuffer::new("second"));
    let tabs_before = app.documents.order().len();
    load_entries(&mut app, &["c.md"]);
    let mut effects = Effects::default();
    preview_one_file(&mut app, &mut effects);
    assert!(app.explorer.preview.is_some(), "preview minted");

    crate::app::update(&mut app, Msg::Key(ctrl('2')), &mut effects);

    assert_eq!(app.documents.order().len(), tabs_before, "no tab moved");
    assert!(app.explorer.preview.is_none());
    assert_eq!(app.active, second, "^2 selects the second real tab");
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
    preview_one_file(&mut app, &mut effects);
    let id = app.explorer.preview.as_ref().expect("preview minted").id;
    let tabs_before = app.documents.order().len();
    // `on_focus_changed` only reacts to an actual TRANSITION — land on the
    // Explorer first (browsing it is what minted the preview above in the
    // first place).
    app.set_focus_pane(Pane::Explorer, &mut effects);

    crate::app::update(&mut app, Msg::Key(ESCAPE), &mut effects);

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
        tabs_before + 1,
        "promotion opens the tab the preview never held"
    );
    assert_eq!(app.active, id, "the promoted document becomes active");
}

#[test]
fn ctrl_b_hiding_the_column_promotes_the_live_preview() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    preview_one_file(&mut app, &mut effects);
    let id = app.explorer.preview.as_ref().expect("preview minted").id;
    app.set_focus_pane(Pane::Explorer, &mut effects);
    assert!(app.splits.left.is_shown(), "column starts visible");

    crate::app::update(&mut app, Msg::Key(ctrl('b')), &mut effects);

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
    preview_one_file(&mut app, &mut effects);
    let id = app.explorer.preview.as_ref().expect("preview minted").id;
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
        tabs_before + 1,
        "the promoted document is the only new tab"
    );
}

#[test]
fn escape_from_the_tabs_pane_has_no_preview_left_to_promote() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    let real = app.active;
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    preview_one_file(&mut app, &mut effects);
    let preview_id = app.explorer.preview.as_ref().expect("preview minted").id;
    let tabs_before = app.documents.order().len();
    app.set_focus_pane(Pane::Explorer, &mut effects);

    crate::app::update(&mut app, Msg::Key(ctrl('t')), &mut effects);
    assert!(
        app.explorer.preview.is_none(),
        "landing on Tabs already discarded it"
    );

    crate::app::update(&mut app, Msg::Key(ESCAPE), &mut effects);

    assert_eq!(app.focus(), Pane::Editor);
    assert!(app.explorer.preview.is_none());
    assert!(
        !app.documents.order().contains(&preview_id),
        "the discarded preview never becomes a tab"
    );
    assert_eq!(app.documents.order().len(), tabs_before);
    assert_eq!(app.active, real, "the surviving real tab stays active");
}

fn pin_the_workspace_to_its_tab_cap(app: &mut App) {
    while app.documents.order().len() < crate::opentabs::limit::MAX_TABS {
        let id = app.open_document(CoreBuffer::new("pinned"));
        app.doc_mut(id).unwrap().pinned = true;
    }
    let active = app.active;
    app.doc_mut(active).unwrap().pinned = true;
}

fn click_editor(app: &mut App, col: u16, row: u16, effects: &mut Effects) {
    let editor = crate::layout::geometry(app.frame_area(), app).editor;
    crate::app::update(
        app,
        Msg::Mouse(crate::pointer::MouseInput {
            kind: crate::pointer::MouseKind::Down(crate::pointer::MouseButton::Left),
            column: editor.x + col,
            row: editor.y + row,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        effects,
    );
}

#[test]
fn a_click_the_tab_cap_refuses_to_promote_still_moves_the_caret_and_reports_the_limit() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/c.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    pin_the_workspace_to_its_tab_cap(&mut app);
    let active = app.active;
    load_entries(&mut app, &["c.md"]);
    let mut effects = Effects::default();
    preview_one_file(&mut app, &mut effects);
    app.set_focus_pane(Pane::Explorer, &mut effects);
    app.sync_view();
    assert!(app.explorer.preview.is_some(), "test setup: preview minted");

    click_editor(&mut app, 3, 0, &mut effects);

    assert_eq!(
        crate::messages::newest_text(&app),
        Some("Tab limit reached \u{2014} close or unpin a tab"),
        "a click that cannot open the previewed file must say why"
    );
    assert_eq!(app.active, active, "the refused file claimed no tab");
    assert!(
        !app.showing_preview(),
        "the editor pane owns the screen once it owns the focus"
    );
    assert_eq!(
        app.active_doc().cursors.primary().position,
        rune_core::coords::BufferOffset(3),
        "the click still lands the caret in the document it can reach"
    );
}
