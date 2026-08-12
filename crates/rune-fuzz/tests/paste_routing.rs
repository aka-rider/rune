#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::{Mem, Vfs};

const OPEN_FILESEARCH: KeyInput = KeyInput {
    code: KeyCode::Char('F'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

fn step(app: &mut App, msg: Msg) {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
    app.sync_view();
}

fn fresh_app() -> App {
    let mem = Arc::new(Mem::new());
    let vfs: Arc<dyn Vfs + Send + Sync> = mem as Arc<dyn Vfs + Send + Sync>;
    App::new(Buffer::new(""), None, vfs, None)
}

/// A frame too small to paint the Explorer column
/// falls the chrome `Pane` back to `Editor` even while file-search owns the
/// keyboard (`app.overlay`). A bracketed paste routes through `focus::
/// target`, not the chrome `Pane`, so it must still land in the query.
#[test]
fn near_zero_resize_then_filesearch_paste_lands_in_query_not_document() {
    let mut app = fresh_app();
    step(&mut app, Msg::Resize(1, 2));
    step(&mut app, Msg::Key(OPEN_FILESEARCH));

    assert!(
        app.filesearch().is_some(),
        "expected file-search to be open after ^F at 1x2"
    );
    assert_eq!(
        app.focus(),
        rune_tui::pane::Pane::Editor,
        "expected the chrome Pane to have fallen back to Editor at 1x2 even \
         though file-search is open"
    );

    step(&mut app, Msg::Paste("hello world".to_string()));

    let query = app
        .filesearch()
        .map(|s| s.query.clone())
        .unwrap_or_default();
    assert_eq!(
        query, "hello world",
        "pasted text should have landed in the file-search query"
    );
    assert_eq!(
        app.active_doc().buffer.content(),
        "",
        "the document must stay untouched by a paste routed to file-search"
    );
}

#[test]
fn normal_size_filesearch_paste_lands_in_query_too() {
    let mut app = fresh_app();
    step(&mut app, Msg::Resize(80, 24));
    step(&mut app, Msg::Key(OPEN_FILESEARCH));
    step(&mut app, Msg::Paste("hello world".to_string()));

    let query = app
        .filesearch()
        .map(|s| s.query.clone())
        .unwrap_or_default();
    assert_eq!(query, "hello world");
    assert_eq!(app.active_doc().buffer.content(), "");
}

#[test]
fn a_paste_swallowed_by_any_focus_target_is_caught() {
    let mut app = fresh_app();
    step(&mut app, Msg::Resize(1, 2));
    step(&mut app, Msg::Key(OPEN_FILESEARCH));

    let before_query = app
        .filesearch()
        .map(|s| s.query.clone())
        .unwrap_or_default();
    let before_doc = app.active_doc().buffer.content().to_string();

    step(&mut app, Msg::Paste("hello world".to_string()));

    let after_query = app
        .filesearch()
        .map(|s| s.query.clone())
        .unwrap_or_default();
    let after_doc = app.active_doc().buffer.content().to_string();

    let doc_grew = after_doc.len() > before_doc.len();
    let query_grew = after_query.len() > before_query.len();

    assert!(
        doc_grew || query_grew,
        "a non-empty paste vanished: neither the document nor the file-search \
         query grew (query {before_query:?} -> {after_query:?}, doc {before_doc:?} -> {after_doc:?})"
    );
}
