//! `App`'s quit-confirm state machine and `Cmd` tagging (moved out of
//! `app.rs` to keep that file under the 500-line budget —
//! every item exercised here (`App`, `update`, `Msg`, `Effects`, `CmdKind`,
//! `keymap` types) is already public, so this needs no crate-internal
//! access `#[cfg(test)]` had).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod dirty_common;

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{App, QuitNegotiation, update};
use rune_tui::commands::clipboard;
use rune_tui::generation::Generation;
use rune_tui::keymap::{KeyCode, KeyInput, Mods, QuitKey};
use rune_tui::runtime::{CmdKind, Effects, Msg, PasteTarget};
use rune_vfs::{Mem, Vfs};

fn test_app() -> App {
    App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
}

fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

#[test]
fn first_quit_press_arms_the_confirm_timer_without_quitting() {
    let mut app = test_app();
    let mut effects = Effects::default();
    let ctrl_c = key(
        KeyCode::Char('c'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );

    update(&mut app, Msg::Key(ctrl_c), &mut effects);

    assert!(!app.should_quit);
    assert_eq!(
        app.quit,
        QuitNegotiation::ConfirmArmed(QuitKey::CtrlC, Generation::ZERO)
    );
    // The quit-confirm timeout is armed directly on `App::timers`, not
    // spawned as its own `Cmd` — no `Effects.cmds` entry for it.
    assert_eq!(effects.cmds.len(), 0);
}

#[test]
fn same_chord_twice_quits() {
    let mut app = test_app();
    let ctrl_c = key(
        KeyCode::Char('c'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );

    let mut effects = Effects::default();
    update(&mut app, Msg::Key(ctrl_c), &mut effects);
    assert!(!app.should_quit);

    let mut effects = Effects::default();
    update(&mut app, Msg::Key(ctrl_c), &mut effects);
    assert!(app.should_quit);
}

#[test]
fn different_quit_chord_re_arms_instead_of_quitting() {
    let mut app = test_app();
    let ctrl_c = key(
        KeyCode::Char('c'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    let ctrl_d = key(
        KeyCode::Char('d'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );

    let mut effects = Effects::default();
    update(&mut app, Msg::Key(ctrl_c), &mut effects);
    assert_eq!(
        app.quit,
        QuitNegotiation::ConfirmArmed(QuitKey::CtrlC, Generation::ZERO)
    );

    let mut effects = Effects::default();
    update(&mut app, Msg::Key(ctrl_d), &mut effects);
    assert!(!app.should_quit, "a different quit chord must not quit");
    assert_eq!(
        app.quit,
        QuitNegotiation::ConfirmArmed(QuitKey::CtrlD, Generation::from_raw(1))
    );
}

#[test]
fn matching_confirm_timeout_clears_pending_quit() {
    let mut app = test_app();
    let ctrl_c = key(
        KeyCode::Char('c'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    let mut effects = Effects::default();
    update(&mut app, Msg::Key(ctrl_c), &mut effects);
    assert_eq!(
        app.quit,
        QuitNegotiation::ConfirmArmed(QuitKey::CtrlC, Generation::ZERO)
    );

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::ConfirmTimeout {
            generation: Generation::ZERO,
        },
        &mut effects,
    );
    assert_eq!(app.quit, QuitNegotiation::Idle);
    assert!(!app.should_quit);
}

#[test]
fn stale_confirm_timeout_is_ignored() {
    let mut app = test_app();
    let ctrl_c = key(
        KeyCode::Char('c'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    let ctrl_d = key(
        KeyCode::Char('d'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    let mut effects = Effects::default();
    update(&mut app, Msg::Key(ctrl_c), &mut effects); // generation 0
    let mut effects2 = Effects::default();
    update(&mut app, Msg::Key(ctrl_d), &mut effects2); // re-arms, generation 1
    assert_eq!(
        app.quit,
        QuitNegotiation::ConfirmArmed(QuitKey::CtrlD, Generation::from_raw(1))
    );

    // The stale generation-0 timeout must not clear the generation-1 pending quit.
    let mut effects3 = Effects::default();
    update(
        &mut app,
        Msg::ConfirmTimeout {
            generation: Generation::ZERO,
        },
        &mut effects3,
    );
    assert_eq!(
        app.quit,
        QuitNegotiation::ConfirmArmed(QuitKey::CtrlD, Generation::from_raw(1))
    );
}

/// Regression for F1: a raw C0 control byte or DEL arriving as
/// `KeyCode::Char` with NO modifier flag at all (the non-Kitty legacy-
/// terminal degradation path, where Ctrl+A IS the literal SOH byte)
/// must never reach the buffer.
#[test]
fn control_bytes_with_no_modifier_are_never_inserted() {
    let mut app = test_app();
    let before = app.active_doc().buffer.content().to_string();

    for raw in ['\u{1}', '\u{7f}', '\u{1b}'] {
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::Key(key(KeyCode::Char(raw), Mods::NONE)),
            &mut effects,
        );
    }

    assert_eq!(
        app.active_doc().buffer.content(),
        before,
        "a raw control byte must never be inserted into the document"
    );
}

#[test]
fn printable_ascii_and_unicode_chars_are_still_insertable() {
    let mut app = test_app();
    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::Key(key(KeyCode::Char('汉'), Mods::NONE)),
        &mut effects,
    );
    assert!(
        app.active_doc().buffer.content().contains('汉'),
        "genuine Unicode text entry must not be blocked by the control-byte guard"
    );
}

#[test]
fn resize_sets_viewport_size_reserving_the_status_row() {
    // Recomputed from `layout::geometry`, not copied from a
    // stale expectation: at 80x24 the footer takes row 23, leaving an
    // 80x23 main/center area (no left pane); the center border takes 2
    // columns (78 wide) and 2 rows for its own top/bottom border, plus 1
    // more row for the title, leaving the editor at 78x20.
    let mut app = test_app();
    let mut effects = Effects::default();
    update(&mut app, Msg::Resize(80, 24), &mut effects);
    assert_eq!(app.active_doc().viewport.width, 78);
    assert_eq!(app.active_doc().viewport.height, 20);
}

#[test]
fn every_cmd_is_tagged_with_its_kind() {
    let vfs = Arc::new(Mem::new());
    let mut app = App::new(
        Buffer::new("x"),
        Some(PathBuf::from("/doc.md")),
        Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
        None,
    );
    let id = app.active;
    dirty_common::force_dirty(&mut app, id);
    let mut effects = Effects::default();
    update(&mut app, Msg::Key(save_key()), &mut effects);
    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::Save);

    let mut app2 = test_app();
    let mut e2 = Effects::default();
    update(
        &mut app2,
        Msg::Key(key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        )),
        &mut e2,
    );
    // The quit-confirm timeout is armed directly on `App::timers`, not
    // spawned as its own `Cmd`.
    assert_eq!(e2.cmds.len(), 0);

    let mut e3 = Effects::default();
    clipboard::paste(&mut e3, PasteTarget::Document(app2.active));
    assert_eq!(e3.cmds.len(), 1);
    assert_eq!(e3.cmds[0].kind(), CmdKind::ClipboardRead);
}

fn save_key() -> KeyInput {
    key(
        KeyCode::Char('s'),
        Mods {
            sup: true,
            ..Mods::NONE
        },
    )
}
