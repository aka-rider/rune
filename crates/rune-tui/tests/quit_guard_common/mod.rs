use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{App, update};
use rune_tui::document::DocumentId;
use rune_tui::guard::{GuardKind, GuardPrompt};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

pub fn test_app() -> App {
    App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
}

pub fn key(code: KeyCode) -> KeyInput {
    KeyInput {
        code,
        mods: Mods::NONE,
    }
}

pub fn ctrl_c() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('c'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    }
}

pub fn press(app: &mut App, input: KeyInput) {
    let mut effects = Effects::default();
    update(app, Msg::Key(input), &mut effects);
}

pub fn guard_kind(app: &App) -> Option<&GuardKind> {
    match &app.guard {
        Some(GuardPrompt { kind, .. }) => Some(kind),
        None => None,
    }
}

pub fn resolved(app: &App, path: &str) -> rune_tui::resolved::ResolvedPath {
    rune_tui::resolved::ResolvedPath::resolve(app.vfs.as_ref(), std::path::Path::new(path))
        .expect("Mem resolves any spelling")
}

pub fn named_dirty_doc(app: &mut App, path: &str) -> DocumentId {
    let id = app.active;
    let path = resolved(app, path);
    app.rebind_document_path(id, path);
    crate::dirty_common::force_dirty(app, id);
    id
}
