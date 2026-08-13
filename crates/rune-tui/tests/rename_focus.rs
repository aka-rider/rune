//! Rename "Done when" tests: the WP2 focus-loss-is-the-single-commit-
//! chokepoint suite — the hoisted blur gate, the invalid-name veto, the
//! Explorer/Tabs focus landings, the outgoing-vs-incoming-document
//! ordering guard, and the close-while-renaming reseed/preserve pair —
//! TODO.md's 500-line budget split of the original `rename.rs`, driven
//! through `rune_fuzz::Session` wherever the flow is key-drivable. The
//! bare-`App` stragglers each need something the session driver
//! deliberately withholds: a `ReadDir`/`ReadFile` `Cmd` run by hand, a raw
//! `Msg::ClipboardRead` injection, or an `Effects`-level timer assertion.

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
    app_with, bound_session, commit_name, ctrl, ctrl_key, plain, plain_key, rename_to, seeded_vfs,
    send, set_name, sup, sup_key, type_new_name, unbound_session,
};

/// The open tab holding `path`, and its position in the tab order — the
/// two facts `Session::switch_tab_by_index`'s real `^t`/arrows/Enter chord
/// needs to walk back to an already-open document.
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

// ── WP2: focus loss is the single commit chokepoint ─────────────────────

/// Leaving the title for the Explorer (`^b`) must commit the pending rename
/// exactly like Enter does — the hoisted blur gate at the top of
/// `pane::handle_global_command` runs before the `FocusExplorer` arm.
#[test]
fn leaving_the_title_for_the_explorer_commits_the_rename() {
    let (mut session, mem) = unbound_session();
    set_name(&mut session, "b");

    assert!(session.key(ctrl_key('b')).is_none());

    assert_eq!(session.app().focus(), Pane::Explorer);
    assert!(
        matches!(session.app().rename, RenameState::Committing { .. }),
        "leaving the title must commit the pending rename"
    );

    assert!(session.deliver().is_none());
    assert_eq!(
        mem.read(Path::new("/root/b.md")).unwrap(),
        b"a content",
        "the committed rename must land"
    );
    assert!(mem.read(Path::new("/root/a.md")).is_err());
}

/// Escape is an unconditional exit even while the typed name is invalid
/// (here, empty — reached by unlocking the gate and clearing everything,
/// since a locked stem always leaves a valid dotfile-shaped name behind):
/// it reverts FIRST, so there is nothing left for `on_blur` to veto, and
/// focus always releases.
#[test]
fn escape_releases_focus_even_when_the_typed_name_is_invalid() {
    let (mut session, mem) = bound_session();
    assert!(session.key(ctrl_key('r')).is_none());
    assert!(session.key(plain_key(KeyCode::Right)).is_none());
    assert!(session.key(ctrl_key('a')).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert_eq!(session.app().title.text(), "");

    assert!(session.key(plain_key(KeyCode::Escape)).is_none());

    assert_eq!(session.app().focus(), Pane::Editor);
    assert_eq!(
        session.app().title.text(),
        "a.md",
        "reverted to the committed name"
    );
    assert!(session.deliver().is_none());
    assert!(session.deliver_db_all().is_none());
    assert_eq!(
        mem.read(Path::new("/root/a.md")).unwrap(),
        b"a content",
        "Escape must never fire a rename"
    );
}

/// An invalid name (here, empty) vetoes the FOCUS change on Enter — the
/// user stays in the title with the reason already in the footer (decision
/// 7), rather than being bounced back to the Editor with an unresolved
/// name.
#[test]
fn an_invalid_name_vetoes_the_focus_change() {
    let (mut session, _mem) = bound_session();
    assert!(session.key(ctrl_key('r')).is_none());
    assert!(session.key(plain_key(KeyCode::Right)).is_none());
    assert!(session.key(ctrl_key('a')).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert_eq!(session.app().title.text(), "");

    assert!(session.key(plain_key(KeyCode::Enter)).is_none());

    assert_eq!(
        session.app().focus(),
        Pane::Title,
        "a refused commit must not release focus"
    );
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("that name can't be used for a file")
    );
}

/// Gotcha 5: a vetoed blur must never block a global command reaching its
/// own arm — ⌘S still triggers a save, and `^c` twice still quits, even
/// while the title holds an unusable (empty) name.
#[test]
fn an_invalid_name_still_lets_the_user_quit_and_save() {
    let (mut session, _mem) = bound_session();
    // A real edit — `trigger_save` gates on `buffer.version() !=
    // saved_version`, which only an actual edit moves.
    assert!(session.key(plain_key(KeyCode::Char('!'))).is_none());
    assert!(session.key(ctrl_key('r')).is_none());
    assert!(session.key(plain_key(KeyCode::Right)).is_none());
    assert!(session.key(ctrl_key('a')).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert_eq!(session.app().title.text(), "");
    assert_eq!(
        session.app().focus(),
        Pane::Title,
        "test setup: the veto leaves focus in the title"
    );

    // ⌘S must still reach `Save`'s own arm rather than being swallowed by
    // the title: the hoisted gate only ever vetoes the FOCUS transition,
    // never the command itself.
    assert!(session.key(sup_key('s')).is_none());
    assert!(
        session.app().active_doc().save_in_flight(),
        "\u{2318}S must still trigger a save even with an unusable name pending"
    );

    // `^c` twice must still reach the quit chord's own arm and complete
    // quit — this document is store-bound, so the unpreserved-dirty Guard
    // gate never intercepts it.
    assert!(session.key(ctrl_key('c')).is_none());
    assert!(session.key(ctrl_key('c')).is_none());
    assert!(session.app().should_quit, "^c^c must still be able to quit");
}

/// WP2.S8 did not strand focus: both the Explorer's `Enter`
/// (`workspace::open_path`, wrapped by `explorer_keys::open_selected`) and
/// the Tabs pane's `Enter` (`opentabs::handle_key`'s `Select` arm) still
/// land focus on the Editor now that `switch_to` itself no longer writes
/// it. Bare-`App`: the Explorer's entries only populate through a
/// `ReadDir` `Cmd` run by hand, which the session driver drops.
#[test]
fn explorer_enter_and_tabs_enter_both_land_focus_on_the_editor() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);

    let mut effects = send(&mut app, ctrl('b'));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::ReadDir)
        .expect("a ReadDir Cmd");
    let msg = cmd.run().expect("a reply");
    send(&mut app, msg);
    assert_eq!(app.focus(), Pane::Explorer);
    let idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "b.md")
        .expect("b.md listed");
    app.explorer.nav.cursor = idx;

    send(&mut app, plain(KeyCode::Enter));
    assert_eq!(
        app.focus(),
        Pane::Editor,
        "Explorer Enter must land focus on the Editor"
    );

    send(&mut app, ctrl('t'));
    assert_eq!(app.focus(), Pane::Tabs);
    app.tabs.nav.cursor = 0;

    send(&mut app, plain(KeyCode::Enter));
    assert_eq!(
        app.focus(),
        Pane::Editor,
        "Tabs Enter must land focus on the Editor"
    );
}

/// Gotcha 6: `^1`-`^0` and `F1` fired from Explorer focus must land focus on
/// the Editor too — the hoisted blur gate at `pane.rs` fires only for
/// `Pane::Title`, so without WP2.S8's explicit `set_focus` in the
/// `TabSwitch`/`Help` arms, the document would switch while focus stayed
/// stranded on the chrome list.
#[test]
fn ctrl_1_and_f1_from_explorer_focus_land_focus_on_the_editor() {
    let (mut session, mem) = bound_session();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    workspace::open_path(session.app_mut(), Path::new("/root/b.md")).expect("open b.md");
    session.app_mut().active_doc_mut().focused = true;

    assert!(session.key(ctrl_key('b')).is_none());
    assert_eq!(session.app().focus(), Pane::Explorer);
    assert!(session.key(ctrl_key('1')).is_none());
    assert_eq!(
        session.app().focus(),
        Pane::Editor,
        "^1 from Explorer focus must land focus on the Editor"
    );

    // `^1` moved focus but never touched the column's own visibility, so
    // it's still shown — `^b` is a genuine toggle now (Enter/Escape
    // rework), so ONE press here would hide it instead of re-focusing the
    // Explorer. Two presses (hide, then show) reach the Explorer reliably
    // regardless of the column's visibility going in.
    assert!(session.key(ctrl_key('b')).is_none());
    assert!(session.key(ctrl_key('b')).is_none());
    assert_eq!(session.app().focus(), Pane::Explorer);
    assert!(session.key(plain_key(KeyCode::F1)).is_none());
    assert_eq!(
        session.app().focus(),
        Pane::Editor,
        "F1 from Explorer focus must land focus on the Editor"
    );
}

/// The ordering guard for decision 8: an uncommitted rename must target the
/// OUTGOING document, never the one about to become active. A different
/// document opening asynchronously (`workspace::open_path_async`, e.g. a
/// ctrl-click on a link) blurs the title — and so fires the pending rename
/// — BEFORE its `Msg::FileOpened` reply reassigns `app.active`. Bare-`App`:
/// the async open's `ReadFile` `Cmd` is run by hand, which the session
/// driver drops.
#[test]
fn an_uncommitted_title_renames_the_outgoing_document_not_the_incoming_one() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/other.md"), b"other content")
        .expect("seed other.md");
    let mut app = app_with(&mem);

    type_new_name(&mut app, "renamed");

    let mut effects = Effects::default();
    workspace::open_path_async(&mut app, Path::new("/root/other.md"), None, &mut effects);
    let read_cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::ReadFile)
        .expect("a ReadFile Cmd");
    let file_opened = read_cmd.run().expect("a reply");
    let mut effects2 = send(&mut app, file_opened);

    // The active document is now the newly opened one...
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/other.md"))
    );
    // ...but the rename Cmd the blur fired targeted the OLD document's own
    // directory and old name, never the new one.
    let rename_cmd = effects2
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("the blur must have fired a rename Cmd for the outgoing document");
    let reply = rename_cmd.run().expect("a reply");
    send(&mut app, reply);

    assert_eq!(
        mem.read(Path::new("/root/renamed.md")).unwrap(),
        b"a content"
    );
    assert!(
        mem.read(Path::new("/root/a.md")).is_err(),
        "the OLD name must be gone"
    );
    assert_eq!(
        mem.read(Path::new("/root/other.md")).unwrap(),
        b"other content",
        "the newly-opened document's own file must be untouched"
    );
}

/// Decision 8's conditional half: a failed Explorer open raises the error
/// banner and must NOT steal the keyboard — focus stays on the Explorer so
/// the user can try a different entry. Bare-`App` for the same `ReadDir`
/// reason as `explorer_enter_and_tabs_enter_both_land_focus_on_the_editor`.
#[test]
fn a_failed_explorer_open_leaves_focus_on_the_explorer() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);

    let mut effects = send(&mut app, ctrl('b'));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::ReadDir)
        .expect("a ReadDir Cmd");
    let msg = cmd.run().expect("a reply");
    send(&mut app, msg);
    assert_eq!(app.focus(), Pane::Explorer);

    let idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "b.md")
        .expect("b.md listed");
    app.explorer.nav.cursor = idx;
    mem.fail_next(rune_vfs::OpKind::Read, std::io::ErrorKind::PermissionDenied);

    send(&mut app, plain(KeyCode::Enter));

    assert_eq!(
        app.focus(),
        Pane::Explorer,
        "a failed open must not steal the keyboard from the Explorer"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some(),
        "the read failure must post an error message"
    );
}

/// The first half of WP2.S8c's guard: closing the active document reseeds
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

// ── WP4: the paste target travels with the request ──────────────────────

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
    let save_effects = send(&mut app, sup('s'));

    // Checked on the very `Effects` the refusal itself produced (dispatch's
    // `after_update` reconciler arms the timer within the same `update`
    // call) — a later, separate read of `should_arm_auto_collapse` would
    // see the timer already armed by that same reconciler and pass either
    // way, regardless of the message's severity.
    let armed = save_effects
        .cmds
        .iter()
        .any(|c| c.kind() == CmdKind::MessagesCollapseTimeout);
    assert!(
        !armed,
        "a save refused for an in-flight rename must never arm the \
         auto-collapse timer"
    );
    assert!(
        messages::is_open(&app),
        "the pane must still be open right after the refusal"
    );
}
