use super::*;

fn press(app: &mut crate::app::App, code: KeyCode, mods: Mods) {
    let mut effects = crate::runtime::Effects::default();
    crate::app::update(
        app,
        crate::runtime::Msg::Key(crate::keymap::KeyInput { code, mods }),
        &mut effects,
    );
}

fn fresh_app() -> crate::app::App {
    crate::app::App::new(
        rune_core::buffer::Buffer::new("hello"),
        None,
        std::sync::Arc::new(rune_vfs::Mem::new()),
        None,
    )
}

#[test]
fn f2_focuses_the_title_for_a_rename() {
    let mut app = fresh_app();
    press(&mut app, KeyCode::F2, Mods::NONE);
    assert_eq!(app.focus(), crate::pane::Pane::Title);
}

#[test]
fn ctrl_r_no_longer_focuses_the_title() {
    let mut app = fresh_app();
    press(&mut app, KeyCode::Char('r'), CTRL);
    assert_ne!(app.focus(), crate::pane::Pane::Title);
}

#[test]
fn every_r_chord_resolves_to_a_replace_command() {
    use crate::binding::resolve_in;
    use crate::keymap::KeyInput;

    let cases = [
        (KeyCode::Char('r'), CTRL, GlobalCommand::ToggleReplace),
        (KeyCode::Char('r'), SUP, GlobalCommand::ToggleReplace),
        (
            KeyCode::Char('R'),
            CTRL,
            GlobalCommand::ToggleProjectReplace,
        ),
        (KeyCode::Char('R'), SUP, GlobalCommand::ToggleProjectReplace),
    ];
    for (code, mods, expected) in cases {
        let key = KeyInput { code, mods };
        assert_eq!(resolve_in(GLOBAL_BINDINGS, key), Some(expected), "{key:?}");
    }
}

#[test]
fn sup_r_no_longer_reaches_reload_through_the_editor_table() {
    use crate::binding::resolve_in;
    use crate::keymap::editor_bindings::EDITOR_BINDINGS;
    use crate::keymap::{Command, KeyInput};

    let sup_r = KeyInput {
        code: KeyCode::Char('r'),
        mods: SUP,
    };
    assert_ne!(resolve_in(EDITOR_BINDINGS, sup_r), Some(Command::Reload));
    assert_eq!(resolve_in(EDITOR_BINDINGS, sup_r), None);
}

#[test]
fn rename_hint_is_labelled_f2() {
    assert_eq!(
        hint_for(GlobalCommand::FocusTitle),
        Some(("F2".to_string(), "rename"))
    );
}
