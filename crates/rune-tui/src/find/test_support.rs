use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use crate::app::App;
use crate::find::FindState;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::pointer::{MouseButton, MouseInput, MouseKind};
use crate::runtime::{Effects, Msg};

pub(crate) const FRAME_W: u16 = 80;
pub(crate) const FRAME_H: u16 = 24;

pub(crate) const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

pub(crate) const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

pub(crate) const ALT: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: false,
};

pub(crate) const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};

pub(crate) fn app_with(content: &str) -> App {
    app_sized(content, FRAME_W, FRAME_H)
}

pub(crate) fn app_sized(content: &str, width: u16, height: u16) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.frame = Some(crate::app::FrameSize::new(width, height));
    app.sync_view();
    app
}

pub(crate) fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

pub(crate) fn char_key(c: char) -> KeyInput {
    key(KeyCode::Char(c), Mods::NONE)
}

pub(crate) fn press(app: &mut App, key: KeyInput) -> Effects {
    let mut effects = Effects::default();
    crate::app::update(app, Msg::Key(key), &mut effects);
    effects
}

pub(crate) fn type_str(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, char_key(c));
    }
}

pub(crate) fn open_find(app: &mut App) {
    press(app, key(KeyCode::Char('f'), CTRL));
}

pub(crate) fn open_replace(app: &mut App) {
    press(app, key(KeyCode::Char('r'), CTRL));
}

pub(crate) fn enter() -> KeyInput {
    key(KeyCode::Enter, Mods::NONE)
}

pub(crate) fn shift_enter() -> KeyInput {
    key(KeyCode::Enter, SHIFT)
}

pub(crate) fn escape() -> KeyInput {
    key(KeyCode::Escape, Mods::NONE)
}

pub(crate) fn tab() -> KeyInput {
    key(KeyCode::Tab, Mods::NONE)
}

pub(crate) fn shift_tab() -> KeyInput {
    key(KeyCode::Tab, SHIFT)
}

pub(crate) fn space() -> KeyInput {
    char_key(' ')
}

pub(crate) fn backspace() -> KeyInput {
    key(KeyCode::Backspace, Mods::NONE)
}

pub(crate) fn up() -> KeyInput {
    key(KeyCode::Up, Mods::NONE)
}

pub(crate) fn down() -> KeyInput {
    key(KeyCode::Down, Mods::NONE)
}

pub(crate) fn find(app: &App) -> &FindState {
    app.find().expect("the find panel is open")
}

pub(crate) fn selection_start(app: &App) -> usize {
    app.active_doc().cursors.primary().selection_start().get()
}

pub(crate) fn click(app: &mut App, column: u16, row: u16) {
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

pub(crate) fn grid(app: &mut App) -> Vec<String> {
    app.sync_view();
    crate::testgrid::grid(app, FRAME_W, FRAME_H)
}

pub(crate) fn panel_top_row(app: &mut App) -> usize {
    grid(app)
        .iter()
        .position(|row| row.contains("\u{256d} Find"))
        .expect("the panel's top border row is on screen")
}

pub(crate) fn bg_at(app: &mut App, x: u16, y: u16) -> Option<ratatui::style::Color> {
    app.sync_view();
    let buf = crate::testgrid::draw(app, FRAME_W, FRAME_H);
    buf.cell((x, y)).and_then(|cell| cell.style().bg)
}

pub(crate) fn occurrences(row: &str, needle: &str) -> Vec<u16> {
    row.match_indices(needle)
        .map(|(byte, _)| row[..byte].chars().count() as u16)
        .collect()
}

pub(crate) fn document_row(app: &mut App, needle: &str) -> (u16, String) {
    let rows = document_rows(app);
    let found = rows.iter().position(|row| row.contains(needle));
    assert!(
        found.is_some(),
        "no document row contains {needle:?}: {rows:?}"
    );
    let y = found.unwrap_or_default();
    (y as u16, rows.get(y).cloned().unwrap_or_default())
}

pub(crate) fn document_rows(app: &mut App) -> Vec<String> {
    let top = panel_top_row(app);
    grid(app).into_iter().take(top).collect()
}
