//! Rename "Done when" tests: the WP2 focus-loss-is-the-single-commit-
//! chokepoint suite — the hoisted blur gate, the invalid-name veto, the
//! Explorer/Tabs focus landings, the outgoing-vs-incoming-document
//! ordering guard, and the close-while-renaming reseed/preserve pair —
//! TODO.md's §1.6 split of the original `rename.rs`. Focus/typing, the
//! refusals, the no-store end-to-end rename, and draft naming live in the
//! sibling `rename_bind.rs`; the collision guard and hazard-1 tests live
//! in `rename_collision.rs`; the store-backed `[R]eplace` path lives in
//! `rename_replace.rs`. All four pull shared fixtures from
//! `rename_common`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;

use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::runtime::{CmdKind, Effects, Msg, PasteTarget};
use rune_tui::workspace;
use rune_vfs::Vfs;

use rename_common::{app_with, app_with_store, ctrl, key, plain, seeded_vfs, send, type_new_name};

// ── WP2: focus loss is the single commit chokepoint ─────────────────────

/// Leaving the title for the Explorer (`^b`) must commit the pending rename
/// exactly like Enter does — the hoisted blur gate at the top of
/// `pane::handle_global_command` runs before the `FocusExplorer` arm.
#[test]
fn leaving_the_title_for_the_explorer_commits_the_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    type_new_name(&mut app, "b");

    let mut effects = send(&mut app, ctrl('b'));

    assert_eq!(app.focus(), Pane::Explorer);
    let cmds: Vec<_> = effects
        .cmds
        .drain(..)
        .filter(|c| c.kind() == CmdKind::Rename)
        .collect();
    assert_eq!(
        cmds.len(),
        1,
        "leaving the title must commit the pending rename"
    );
    assert!(matches!(app.rename, RenameState::Committing { .. }));
}

/// Escape is an unconditional exit even while the typed name is invalid
/// (here, empty — reached by unlocking the gate and clearing everything,
/// since a locked stem always leaves a valid dotfile-shaped name behind):
/// it reverts FIRST, so there is nothing left for `on_blur` to veto, and
/// focus always releases.
#[test]
fn escape_releases_focus_even_when_the_typed_name_is_invalid() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    send(&mut app, ctrl('r'));
    send(&mut app, plain(KeyCode::Right));
    send(&mut app, ctrl('a'));
    send(&mut app, plain(KeyCode::Backspace));
    assert_eq!(app.title.text(), "");

    let effects = send(&mut app, plain(KeyCode::Escape));

    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.title.text(), "a.md", "reverted to the committed name");
    assert!(
        !effects.cmds.iter().any(|c| c.kind() == CmdKind::Rename),
        "Escape must never fire a rename"
    );
}

/// An invalid name (here, empty) vetoes the FOCUS change on Enter — the
/// user stays in the title with the reason already in the footer (decision
/// 7), rather than being bounced back to the Editor with an unresolved
/// name.
#[test]
fn an_invalid_name_vetoes_the_focus_change() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    send(&mut app, ctrl('r'));
    send(&mut app, plain(KeyCode::Right));
    send(&mut app, ctrl('a'));
    send(&mut app, plain(KeyCode::Backspace));
    assert_eq!(app.title.text(), "");

    send(&mut app, plain(KeyCode::Enter));

    assert_eq!(
        app.focus(),
        Pane::Title,
        "a refused commit must not release focus"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("that name can't be used for a file")
    );
}

/// Gotcha 5: a vetoed blur must never block a global command reaching its
/// own arm — ⌘S still triggers a save, and `^c` twice still quits, even
/// while the title holds an unusable (empty) name.
#[test]
fn an_invalid_name_still_lets_the_user_quit_and_save() {
    let mem = seeded_vfs();
    let (mut app, _rx) = app_with_store(&mem);
    // A real edit, not `mark_dirty_from_hydration` — `trigger_save` gates on
    // `buffer.version() != saved_version`, which only an actual edit moves.
    send(&mut app, plain(KeyCode::Char('!')));
    send(&mut app, ctrl('r'));
    send(&mut app, plain(KeyCode::Right));
    send(&mut app, ctrl('a'));
    send(&mut app, plain(KeyCode::Backspace));
    assert_eq!(app.title.text(), "");
    assert_eq!(
        app.focus(),
        Pane::Title,
        "test setup: the veto leaves focus in the title"
    );

    // ⌘S must still reach `Save`'s own arm rather than being swallowed by
    // the title: the hoisted gate only ever vetoes the FOCUS transition,
    // never the command itself.
    let cmd_s = Mods {
        sup: true,
        ..Mods::NONE
    };
    send(&mut app, key(KeyCode::Char('s'), cmd_s));
    assert!(
        app.active_doc().save_in_flight,
        "\u{2318}S must still trigger a save even with an unusable name pending"
    );

    // `^c` twice must still reach the quit chord's own arm and complete
    // quit — this document is store-bound, so the unpreserved-dirty Guard
    // gate never intercepts it.
    send(&mut app, ctrl('c'));
    send(&mut app, ctrl('c'));
    assert!(app.should_quit, "^c^c must still be able to quit");
}

/// WP2.S8 did not strand focus: both the Explorer's `Enter`
/// (`workspace::open_path`, wrapped by `explorer_keys::open_selected`) and
/// the Tabs pane's `Enter` (`opentabs::handle_key`'s `Select` arm) still
/// land focus on the Editor now that `switch_to` itself no longer writes
/// it.
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
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);
    workspace::open_path(&mut app, Path::new("/root/b.md"));

    send(&mut app, ctrl('b'));
    assert_eq!(app.focus(), Pane::Explorer);
    send(&mut app, ctrl('1'));
    assert_eq!(
        app.focus(),
        Pane::Editor,
        "^1 from Explorer focus must land focus on the Editor"
    );

    send(&mut app, ctrl('b'));
    assert_eq!(app.focus(), Pane::Explorer);
    send(&mut app, plain(KeyCode::F1));
    assert_eq!(
        app.focus(),
        Pane::Editor,
        "F1 from Explorer focus must land focus on the Editor"
    );
}

/// The ordering guard for decision 8: an uncommitted rename must target the
/// OUTGOING document, never the one about to become active. A different
/// document opening asynchronously (`workspace::open_path_async`, e.g. a
/// ctrl-click on a link) blurs the title — and so fires the pending rename
/// — BEFORE its `Msg::FileOpened` reply reassigns `app.active`.
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
/// the user can try a different entry.
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
        app.modal.is_some(),
        "the read failure must raise the error banner"
    );
}

/// The first half of WP2.S8c's guard: closing the active document reseeds
/// the title from the document that becomes active in its place.
#[test]
fn closing_a_tab_reseeds_the_title_from_the_new_active_document() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);
    let first = app.active;
    workspace::open_path(&mut app, Path::new("/root/b.md"));
    let second = app.active;
    assert_ne!(first, second, "test setup: two distinct documents");

    workspace::close_now(&mut app, second);

    assert_eq!(app.active, first);
    assert_eq!(
        app.title.text(),
        "a.md",
        "the title must reseed from the new active document"
    );
}

/// The second half of WP2.S8c's guard: `close_now` is the one active-
/// document reseed with no blur in front of it — an async close landing for
/// the very document being renamed must leave the typed name alone rather
/// than silently overwrite it.
#[test]
fn closing_a_background_tab_while_renaming_leaves_the_typed_name_alone() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);
    // A second document, so the last-document floor doesn't refuse.
    workspace::open_path(&mut app, Path::new("/root/b.md"));
    let first_tab = app.tabs.order[0];
    workspace::switch_to(&mut app, first_tab);
    let victim = app.active;

    type_new_name(&mut app, "zzz");

    // An async close for the very document being renamed lands with no
    // blur in front of it (mirrors `materialize_ack::close_if_pending`'s own
    // shape) — the typed name must survive untouched.
    workspace::close_now(&mut app, victim);

    assert_eq!(
        app.focus(),
        Pane::Title,
        "focus must not be silently displaced"
    );
    assert_eq!(
        app.title.text(),
        "zzz.md",
        "the typed name must survive an async close of the document being renamed"
    );
}

// ── WP4: the paste target travels with the request ──────────────────────

/// The latent bug decision 11 fixes: a `Msg::ClipboardRead` targeted at a
/// specific document (captured when the paste was requested) must land on
/// THAT document even after the active document has since changed — never
/// on whatever happens to be active by the time the reply arrives.
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
/// write into a field the user has since left.
#[test]
fn a_title_targeted_paste_arriving_after_focus_left_the_title_is_dropped() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    assert_eq!(app.focus(), Pane::Title);
    send(&mut app, plain(KeyCode::Escape));
    assert_eq!(app.focus(), Pane::Editor);

    send(
        &mut app,
        Msg::ClipboardRead {
            text: "late".to_string(),
            target: PasteTarget::Title,
        },
    );

    assert_eq!(
        app.title.text(),
        "a.md",
        "a title-targeted paste arriving after focus left must be dropped"
    );
}
