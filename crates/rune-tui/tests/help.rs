//! Tests for the Help virtual document — minted once, toggled
//! via `F1`, read-only, and rendered in the Open Tabs pane.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::document::ReadOnly;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::registry::{self, CommandId};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_tui::workspace;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_with(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn f1() -> KeyInput {
    KeyInput {
        code: KeyCode::F1,
        mods: Mods::NONE,
    }
}

fn frame_text(app: &App) -> String {
    testgrid::grid(app, WIDTH, HEIGHT).concat()
}

/// `F1` twice mints exactly one Help document — the second press must not
/// duplicate it ("press twice -> documents.len() grows by
/// exactly 1 across both presses").
#[test]
fn f1_twice_creates_exactly_one_help_document() {
    let mut app = app_with("hello");
    let before = app.documents.len();

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(f1()), &mut effects);
    let after_first = app.documents.len();

    let mut effects2 = Effects::default();
    app::update(&mut app, Msg::Key(f1()), &mut effects2);
    let after_second = app.documents.len();

    assert_eq!(after_first, before + 1, "first F1 must mint the help doc");
    assert_eq!(
        after_second,
        before + 1,
        "second F1 must not mint a duplicate; documents.len() grew by exactly 1 across both presses"
    );
}

#[test]
fn help_content_covers_every_registry_section() {
    let mut app = app_with("hello");
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(f1()), &mut effects);

    let content = app.active_doc().buffer.content().to_string();
    assert!(
        content.contains("## Global"),
        "missing ## Global:\n{content}"
    );

    type SectionPredicate = (&'static str, fn(CommandId) -> bool);
    let sections: [SectionPredicate; 7] = [
        ("Global", |id| matches!(id, CommandId::Global(_))),
        ("Explorer", |id| {
            matches!(id, CommandId::Explorer(_) | CommandId::ExplorerSearch(_))
        }),
        ("File Search", |id| matches!(id, CommandId::FileSearch(_))),
        ("Open Tabs", |id| matches!(id, CommandId::Tabs(_))),
        ("Editor", |id| matches!(id, CommandId::Editor(_))),
        ("Diff View", |id| matches!(id, CommandId::Diff(_))),
        ("Palette", |id| matches!(id, CommandId::Palette(_))),
    ];
    for (title, pick) in sections {
        assert!(
            registry::all()
                .iter()
                .filter(|row| pick(row.id))
                .any(|row| content.contains(row.help)),
            "expected a {title} help label in:\n{content}"
        );
    }
}

/// The Help document rejects edits: a printable key while it's active
/// leaves the buffer unchanged, and `is_dirty()` stays false.
#[test]
fn help_document_rejects_edits_and_never_goes_dirty() {
    let mut app = app_with("hello");
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(f1()), &mut effects);
    assert_eq!(
        app.active_doc().read_only,
        ReadOnly::Always,
        "F1 must mint ReadOnly::Always — Reading has an editable form to \
         return to via ⌃P, which would make this document editable again"
    );

    let content_before = app.active_doc().buffer.content().to_string();
    let printable = KeyInput {
        code: KeyCode::Char('x'),
        mods: Mods::NONE,
    };
    let mut effects2 = Effects::default();
    app::update(&mut app, Msg::Key(printable), &mut effects2);

    assert_eq!(
        app.active_doc().buffer.content(),
        content_before,
        "a printable key must not mutate a read-only document"
    );
    assert!(
        !app.active_doc().dirty_for_render(),
        "a rejected edit must never mark the help doc dirty"
    );
}

/// `F1` while Help is active returns to the document that was active right
/// before Help was toggled on.
#[test]
fn f1_while_help_active_returns_to_the_previous_document() {
    let mut app = app_with("hello");
    let original = app.active;

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(f1()), &mut effects);
    let help_id = app.active;
    assert_ne!(help_id, original);

    let mut effects2 = Effects::default();
    app::update(&mut app, Msg::Key(f1()), &mut effects2);

    assert_eq!(
        app.active, original,
        "F1 must switch back to the original doc"
    );
    assert!(
        app.documents.contains_key(&help_id),
        "toggling off must not close the help doc, only switch away from it"
    );
}

/// Closing the document Help would otherwise switch back to must not
/// panic: toggling Help off lands on any other live document instead.
#[test]
fn closing_the_previous_document_then_toggling_help_lands_somewhere_live() {
    let mut app = app_with("hello");
    let original = app.active;
    let second = app.open_document(Buffer::new("second"));

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(f1()), &mut effects);
    let help_id = app.active;
    assert_eq!(app.help_return_to, Some(original));

    // Close `original` while it's not active (help is) — a clean document,
    // so this closes immediately rather than arming the Guard.
    let _ = workspace::close_now(&mut app, original, &mut Effects::default());
    assert!(!app.documents.contains_key(&original));
    assert_eq!(app.documents.len(), 2, "help doc + second must remain");

    let mut effects2 = Effects::default();
    app::update(&mut app, Msg::Key(f1()), &mut effects2);

    assert_ne!(app.active, help_id, "must have switched away from help");
    assert!(
        app.documents.contains_key(&app.active),
        "must land on a live document"
    );
    assert_eq!(app.active, second);
}

/// The help tab renders in the Open Tabs pane with the display name
/// "Help".
#[test]
fn help_tab_renders_in_open_tabs_pane_with_the_name_help() {
    let mut app = app_with("hello");
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(f1()), &mut effects);

    app.splits.left.show();
    app.set_focus_pane(Pane::Tabs, &mut Effects::default());
    app.sync_view();

    let text = frame_text(&app);
    assert!(
        text.contains("Help"),
        "expected \"Help\" in the Tabs pane:\n{text}"
    );
}
