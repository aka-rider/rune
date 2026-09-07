use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use crate::app::App;
use crate::find::FindState;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::pointer::{MouseButton, MouseInput, MouseKind};
use crate::runtime::{Effects, Msg};

pub(super) const FRAME_W: u16 = 80;
pub(super) const FRAME_H: u16 = 24;

pub(super) const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

pub(super) const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

pub(super) const ALT: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: false,
};

pub(super) const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};

pub(super) fn app_with(content: &str) -> App {
    app_sized(content, FRAME_W, FRAME_H)
}

pub(super) fn app_sized(content: &str, width: u16, height: u16) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.frame = Some(crate::app::FrameSize::new(width, height));
    app.sync_view();
    app
}

pub(super) fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

pub(super) fn char_key(c: char) -> KeyInput {
    key(KeyCode::Char(c), Mods::NONE)
}

pub(super) fn press(app: &mut App, key: KeyInput) -> Effects {
    let mut effects = Effects::default();
    crate::app::update(app, Msg::Key(key), &mut effects);
    effects
}

pub(super) fn type_str(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, char_key(c));
    }
}

pub(super) fn open_find(app: &mut App) {
    press(app, key(KeyCode::Char('f'), CTRL));
}

pub(super) fn open_replace(app: &mut App) {
    press(app, key(KeyCode::Char('r'), CTRL));
}

pub(super) fn enter() -> KeyInput {
    key(KeyCode::Enter, Mods::NONE)
}

pub(super) fn shift_enter() -> KeyInput {
    key(KeyCode::Enter, SHIFT)
}

pub(super) fn escape() -> KeyInput {
    key(KeyCode::Escape, Mods::NONE)
}

pub(super) fn tab() -> KeyInput {
    key(KeyCode::Tab, Mods::NONE)
}

pub(super) fn shift_tab() -> KeyInput {
    key(KeyCode::Tab, SHIFT)
}

pub(super) fn space() -> KeyInput {
    char_key(' ')
}

pub(super) fn backspace() -> KeyInput {
    key(KeyCode::Backspace, Mods::NONE)
}

pub(super) fn up() -> KeyInput {
    key(KeyCode::Up, Mods::NONE)
}

pub(super) fn down() -> KeyInput {
    key(KeyCode::Down, Mods::NONE)
}

pub(super) fn find(app: &App) -> &FindState {
    app.find().expect("the find panel is open")
}

pub(super) fn selection_start(app: &App) -> usize {
    app.active_doc().cursors.primary().selection_start().get()
}

pub(super) fn click(app: &mut App, column: u16, row: u16) {
    let mut effects = Effects::default();
    crate::app::update(
        app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column,
            row,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
}

pub(super) fn grid(app: &mut App) -> Vec<String> {
    app.sync_view();
    crate::testgrid::grid(app, FRAME_W, FRAME_H)
}

pub(super) fn panel_top_row(app: &mut App) -> usize {
    grid(app)
        .iter()
        .position(|row| row.contains("\u{256d} Find"))
        .expect("the panel's top border row is on screen")
}
