use rune_tui::app::{App, update};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};

pub fn save_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('s'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    }
}

pub fn press_save(app: &mut App) -> Effects {
    let mut effects = Effects::default();
    update(app, Msg::Key(save_key()), &mut effects);
    effects
}

pub fn settle_cmds(app: &mut App, effects: Effects) {
    for cmd in effects.cmds {
        if let Some(msg) = cmd.run() {
            let mut next = Effects::default();
            update(app, msg, &mut next);
            settle_cmds(app, next);
        }
    }
}
