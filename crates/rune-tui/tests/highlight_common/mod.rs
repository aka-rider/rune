//! Shared fixtures for the `highlight_*` sibling test files: build an `App`
//! around a fresh buffer, and type one harmless character at the end of the
//! active document's buffer through the real `app::update` chokepoint (the
//! standard way these tests trigger a highlight `Cmd` without reaching for
//! the private `highlight::schedule_highlight` directly). `#![allow(
//! dead_code)]` because each consumer binary only calls a subset of these —
//! the rest would otherwise trip `-D warnings`' dead-code lint in that
//! particular binary.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

pub fn app_for(content: &str, path: &str) -> App {
    App::new(
        Buffer::new(content),
        Some(PathBuf::from(path)),
        Arc::new(Mem::new()),
        None,
    )
}

/// Types one harmless character at the END of the active document's buffer
/// (plan WP6.S5) through the real `app::update` chokepoint, bumping its
/// buffer version so `App::update`'s own before/after gate schedules a
/// highlight `Cmd` — mirrors how a real keystroke does it, without needing
/// the private `highlight::schedule_highlight` directly. The cursor is moved
/// to the very end first (rather than wherever `App::new` put it) so the
/// edit is a pure append. Scheduling refreshes the block tree itself, so an
/// edit BEFORE a fence is equally safe — the sibling regression test below
/// covers exactly that.
pub fn type_one_char_at_end(app: &mut App, effects: &mut Effects) {
    let id = app.active;
    let end = app.doc(id).expect("doc").buffer.content().len();
    app.doc_mut(id).expect("doc").cursors = CursorSet::new(end);
    app::update(
        app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('!'),
            mods: Mods::NONE,
        }),
        effects,
    );
}
