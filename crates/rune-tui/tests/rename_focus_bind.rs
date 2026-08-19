#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;

use rune_tui::keymap::KeyCode;
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::runtime::{CmdKind, Effects};
use rune_tui::workspace;
use rune_vfs::{Vfs, VfsTestExt};

use rename_common::{
    app_with, bound_session, ctrl, ctrl_key, plain, plain_key, seeded_vfs, send, set_name, sup_key,
    type_new_name, unbound_session,
};

// ── Focus loss is the single commit chokepoint ─────────────────────

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

/// Rename did not strand focus: both the Explorer's `Enter`
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
/// `Pane::Title`, so without an explicit `set_focus` in the
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
