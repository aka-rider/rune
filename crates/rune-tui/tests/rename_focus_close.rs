#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;

use rune_tui::document::DocumentId;
use rune_tui::keymap::KeyCode;
use rune_tui::messages;
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::runtime::{CmdKind, Effects, Msg, PasteTarget};
use rune_tui::workspace;
use rune_vfs::Vfs;

use rename_common::{
    app_with, bound_session, commit_name, ctrl, plain, plain_key, rename_to, seeded_vfs, send,
    set_name, sup, sup_key, unbound_session,
};

fn tab_for(session: &rune_fuzz::Session, path: &str) -> (DocumentId, usize) {
    let id = session
        .app()
        .documents
        .iter()
        .find(|(_, doc)| doc.file_path.as_deref() == Some(Path::new(path)))
        .map(|(&id, _)| id)
        .expect("the document is open");
    let index = session
        .app()
        .documents
        .order()
        .iter()
        .position(|&t| t == id)
        .expect("the document has a tab");
    (id, index)
}

/// The first half of the close-while-renaming guard: closing the active document reseeds
/// the title from the document that becomes active in its place.
#[test]
fn closing_a_tab_reseeds_the_title_from_the_new_active_document() {
    let (mut session, mem) = bound_session();
    let first = session.app().active;
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    workspace::open_path(session.app_mut(), Path::new("/root/b.md")).expect("open b.md");
    let second = session.app().active;
    assert_ne!(first, second, "test setup: two distinct documents");

    let _ = workspace::close_now(session.app_mut(), second, &mut Effects::default());

    assert_eq!(session.app().active, first);
    assert_eq!(
        session.app().title.text(),
        "a.md",
        "the title must reseed from the new active document"
    );
}

/// `close_now` is the one active-document reseed with no blur in front of
/// it. Closing a document OTHER than the active one leaves `app.active`
/// untouched, so it must not disturb a name being typed — that is what the
/// `active_changed` guard buys.
///
/// The companion case, closing the ACTIVE document while renaming, is
/// covered by `an_async_close_while_renaming_never_retargets_the_rename_at_
/// the_neighbour` below and must behave the OPPOSITE way: it reseeds, since
/// the field would otherwise describe a document that no longer exists.
#[test]
fn closing_a_background_tab_while_renaming_leaves_the_typed_name_alone() {
    let (mut session, mem) = bound_session();
    // A second document, so the last-document floor doesn't refuse.
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    workspace::open_path(session.app_mut(), Path::new("/root/b.md")).expect("open b.md");
    session.app_mut().active_doc_mut().focused = true;
    let (background, _) = tab_for(&session, "/root/b.md");
    let (renaming, index) = tab_for(&session, "/root/a.md");
    assert!(session.switch_tab_by_index(index).is_none());
    assert_eq!(session.app().active, renaming);

    set_name(&mut session, "zzz");

    // An async close for some OTHER document lands with no blur in front of
    // it (mirrors `materialize_ack::close_if_pending`'s own shape).
    let _ = workspace::close_now(session.app_mut(), background, &mut Effects::default());

    assert_eq!(
        session.app().focus(),
        Pane::Title,
        "focus must not be silently displaced"
    );
    assert_eq!(
        session.app().active,
        renaming,
        "closing a background tab must not move the active document"
    );
    assert_eq!(
        session.app().title.text(),
        "zzz.md",
        "the typed name must survive an async close of an unrelated document"
    );
}

// ── The paste target travels with the request ──────────────────────

/// The latent bug decision 11 fixes: a `Msg::ClipboardRead` targeted at a
/// specific document (captured when the paste was requested) must land on
/// THAT document even after the active document has since changed — never
/// on whatever happens to be active by the time the reply arrives.
/// Bare-`App`: the reply is a raw injected `Msg` the session driver has no
/// title/document-targeted counterpart for.
#[test]
fn a_document_targeted_clipboard_read_lands_on_its_captured_document_even_after_the_active_document_changes()
 {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);
    let first = app.active;

    // The user switches to a different document while a paste requested
    // FROM `first` is still in flight.
    workspace::open_path(&mut app, Path::new("/root/b.md"));
    let second = app.active;
    assert_ne!(first, second, "test setup: two distinct documents");

    send(
        &mut app,
        Msg::ClipboardRead {
            text: "X".to_string(),
            target: PasteTarget::Document(first),
        },
    );

    assert!(
        app.doc(first).unwrap().buffer.content().contains('X'),
        "the reply must land on the document it was requested for"
    );
    assert_eq!(
        app.doc(second).unwrap().buffer.content(),
        "b content",
        "the now-active document must be untouched by a reply targeted elsewhere"
    );
}

/// `title::keys::paste` no-ops unless the title still has focus: `pbpaste`
/// runs on its own thread and can take a while, and a late reply must not
/// write into a field the user has since left. Bare-`App` for the same
/// injected-reply reason as the document-targeted case above.
#[test]
fn a_title_targeted_paste_arriving_after_focus_left_the_title_is_dropped() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    assert_eq!(app.focus(), Pane::Title);
    send(&mut app, plain(KeyCode::Escape));
    assert_eq!(app.focus(), Pane::Editor);

    let target_doc = app.active;
    send(
        &mut app,
        Msg::ClipboardRead {
            text: "late".to_string(),
            target: PasteTarget::Title(target_doc),
        },
    );

    assert_eq!(
        app.title.text(),
        "a.md",
        "a title-targeted paste arriving after focus left must be dropped"
    );
}

/// Regression: an async close of the document being renamed must never let
/// the pending rename retarget whatever document becomes active in its
/// place.
///
/// `close_now` moves `app.active` to a neighbour but deliberately skips the
/// title reseed while the title holds focus, so the field keeps the name
/// typed for the closed document. `rename::begin` then resolves its subject
/// from the live `app.active` — the neighbour — and renames the wrong file
/// through the real VFS with nothing surfaced.
#[test]
fn an_async_close_while_renaming_never_retargets_the_rename_at_the_neighbour() {
    let (mut session, mem) = unbound_session();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    workspace::open_path(session.app_mut(), Path::new("/root/b.md")).expect("open b.md");
    session.app_mut().active_doc_mut().focused = true;
    let (victim, index) = tab_for(&session, "/root/a.md");
    assert!(session.switch_tab_by_index(index).is_none());
    assert_eq!(session.app().active, victim);

    set_name(&mut session, "zzz");

    // The async close lands with no blur in front of it.
    let _ = workspace::close_now(session.app_mut(), victim, &mut Effects::default());

    // Whatever the field still holds, releasing focus must not rename the
    // surviving neighbour to it.
    assert!(session.key(plain_key(KeyCode::Enter)).is_none());
    assert!(session.deliver().is_none());

    assert!(
        mem.read(Path::new("/root/b.md")).is_ok(),
        "the surviving neighbour must keep its own name"
    );
    assert!(
        mem.read(Path::new("/root/zzz.md")).is_err(),
        "no file may be created under the name typed for the closed document"
    );
}

/// Regression: a save must not be issued while a rename is in flight.
///
/// `rename::begin` already refuses when `save_in_flight`, but the reverse
/// direction had no guard. `save_cmd` captures the document's path at
/// trigger time, while the rebind to the new path only happens once the
/// rename ack lands — so a ⌘S between the two writes the edited content
/// back to the OLD path, resurrecting a file the rename was in the middle
/// of moving away from and leaving the new name holding stale bytes.
#[test]
fn a_save_is_refused_while_a_rename_is_in_flight() {
    let (mut session, mem) = unbound_session();

    commit_name(&mut session, "b");
    assert!(matches!(
        session.app().rename,
        RenameState::Committing { .. }
    ));

    // The user edits and hits save before the rename ack lands.
    assert!(session.key(plain_key(KeyCode::Char('X'))).is_none());
    assert!(session.key(sup_key('s')).is_none());
    assert!(
        !session.app().active_doc().save_in_flight(),
        "no save may be issued against the pre-rename path while the rename is in flight"
    );

    // The rename lands; the old name must be gone, not resurrected.
    assert!(session.deliver().is_none());
    assert!(
        mem.read(Path::new("/root/a.md")).is_err(),
        "the pre-rename path must not be resurrected by a racing save"
    );
}

/// The save-refused-during-rename message (finding 2) must stay on screen
/// until the user dismisses it: nothing was written, so an auto-collapsing
/// `warn` would leave the refusal with no trace once its 5s window elapses.
/// Bare-`App`: whether the auto-collapse timer was armed is only visible on
/// the very `Effects` the refusal produced.
#[test]
fn a_save_refused_during_a_rename_leaves_the_message_pane_open() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    let mut effects = rename_to(&mut app, "b");
    effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");
    assert!(matches!(app.rename, RenameState::Committing { .. }));

    send(&mut app, plain(KeyCode::Char('X')));
    send(&mut app, sup('s'));

    // Checked immediately after the refusal's own `update` call returns
    // (dispatch's `after_update` reconciler arms the timer within that same
    // call, directly on `App::timers`, before anything else can run) — a
    // later, separate check after some OTHER action would see the timer
    // already armed by that later action's own reconciler and pass either
    // way, regardless of the message's severity.
    assert!(
        !messages::is_collapse_armed(&app),
        "a save refused for an in-flight rename must never arm the \
         auto-collapse timer"
    );
    assert!(
        messages::is_open(&app),
        "the pane must still be open right after the refusal"
    );
}
