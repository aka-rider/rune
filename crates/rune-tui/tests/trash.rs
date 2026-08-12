//! WP2.S8 "Done when" tests for `⌘⌫`/`^⌫` trash: the guard raise/cancel,
//! every refusal (dirty, pathless, directory), the confirm's `Cmd`
//! enqueue, `Msg::TrashDone`'s close/keep-open/error/stale-generation
//! branches (including the async A4 dirty-at-reply and guard-at-reply
//! cases), and the inherited exact-path-match limitation.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod dirty_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::guard::GuardKind;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::messages;
use rune_tui::messages::Severity;
use rune_tui::pane::Pane;
use rune_tui::runtime::{CmdKind, Effects, Msg};

use rune_vfs::{Mem, Vfs};

fn seeded_vfs() -> Arc<Mem> {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/a.md"), b"a content")
        .expect("seed a.md");
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    mem.save_atomic(Path::new("/root/sub/c.md"), b"c content")
        .expect("seed sub/c.md");
    mem
}

/// An `App` whose active document is `/root/a.md`, no store bound — the
/// no-store `Cmd` route the trash flow always takes (plan: rune-db gets no
/// purge path).
fn app_with(mem: &Arc<Mem>) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new("a content"),
        Some(PathBuf::from("/root/a.md")),
        vfs,
        None,
    );
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();
    app
}

fn send(app: &mut App, msg: Msg) -> Effects {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
    effects
}

fn key(code: KeyCode, mods: Mods) -> Msg {
    Msg::Key(KeyInput { code, mods })
}

fn sup_backspace() -> Msg {
    key(
        KeyCode::Backspace,
        Mods {
            sup: true,
            ..Mods::NONE
        },
    )
}

fn escape() -> Msg {
    key(KeyCode::Escape, Mods::NONE)
}

fn yes() -> Msg {
    key(KeyCode::Char('y'), Mods::NONE)
}

/// `^b` through the real `update` (reveals the active document's own
/// file), then runs the one `ReadDir` `Cmd` it enqueues and delivers its
/// `Msg::DirLoaded` reply synchronously — the same two-step production
/// performs across the `Cmd` thread boundary.
fn load_explorer(app: &mut App) {
    let effects = send(
        app,
        key(
            KeyCode::Char('b'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        ),
    );
    assert_eq!(effects.cmds.len(), 1, "^b must enqueue exactly one Cmd");
    assert_eq!(effects.cmds[0].kind(), CmdKind::ReadDir);
    let mut effects = effects;
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    send(app, msg);
}

/// `⌘⌫` on a clean, named document raises the Trash guard and the footer
/// shows the confirm prompt naming the file.
#[test]
fn clean_named_doc_raises_the_guard_and_names_the_file() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    send(&mut app, sup_backspace());

    assert!(matches!(
        app.guard,
        Some(ref p) if matches!(p.kind, GuardKind::Trash { ref path } if path == Path::new("/root/a.md"))
    ));
    let text = rune_tui::footer::footer_text(&app);
    assert!(text.contains("a.md"), "footer must name the file: {text:?}");
    assert!(text.contains("[Y]es"));
}

/// `Esc` cancels the trash guard with "trash cancelled".
#[test]
fn escape_cancels_with_trash_cancelled() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    send(&mut app, sup_backspace());
    send(&mut app, escape());

    assert!(app.guard.is_none());
    assert_eq!(messages::newest_text(&app), Some("trash cancelled"));
}

/// A dirty document refuses the trash outright — no guard raised, an error
/// posted instead.
#[test]
fn dirty_doc_is_refused_with_no_guard() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let id = app.active;
    dirty_common::force_dirty(&mut app, id);

    send(&mut app, sup_backspace());

    assert!(app.guard.is_none());
    assert_eq!(
        messages::newest(&app).map(|m| m.severity),
        Some(Severity::Error)
    );
}

/// A pathless draft has nothing to trash.
#[test]
fn pathless_draft_is_refused() {
    let mem = Arc::new(Mem::new());
    let vfs: Arc<dyn Vfs + Send + Sync> = mem as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new("draft"), None, vfs, None);
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();

    send(&mut app, sup_backspace());

    assert!(app.guard.is_none());
    assert_eq!(
        messages::newest(&app).map(|m| m.severity),
        Some(Severity::Error)
    );
}

/// Explorer focus with a directory selected is refused.
#[test]
fn explorer_directory_selection_is_refused() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    assert_eq!(app.focus(), Pane::Explorer);
    let idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.kind == rune_vfs::FileKind::Dir)
        .expect("sub is listed");
    app.explorer.nav.cursor = idx;

    send(&mut app, sup_backspace());

    assert!(app.guard.is_none());
    assert_eq!(
        messages::newest(&app).map(|m| m.severity),
        Some(Severity::Error)
    );
}

/// Explorer focus with a file selected carries THAT file's path, not the
/// active document's.
#[test]
fn explorer_file_selection_carries_that_files_path() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    let idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "b.md")
        .expect("b.md is listed");
    app.explorer.nav.cursor = idx;

    send(&mut app, sup_backspace());

    assert!(matches!(
        app.guard,
        Some(ref p) if matches!(p.kind, GuardKind::Trash { ref path } if path == Path::new("/root/b.md"))
    ));
}

/// `y` enqueues exactly one `Trash` `Cmd`.
#[test]
fn yes_enqueues_a_trash_cmd() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    send(&mut app, sup_backspace());
    let effects = send(&mut app, yes());

    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::Trash);
    assert!(app.guard.is_none());
}

/// A second `⌘⌫`+`y` on the same still-clean doc while the first trash
/// `Cmd` is in flight is refused (single-flight): no second `Cmd` is
/// enqueued, no guard is raised, an error is posted — and the first
/// request's reply still lands normally once it arrives.
#[test]
fn second_trash_while_one_in_flight_is_refused() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let closing_id = app.active;
    send(&mut app, sup_backspace());
    let mut effects = send(&mut app, yes());
    assert_eq!(effects.cmds.len(), 1);
    let cmd = effects.cmds.remove(0);

    send(&mut app, sup_backspace());

    assert!(
        app.guard.is_none(),
        "a second trash request must not raise a guard while one is in flight"
    );
    assert_eq!(
        messages::newest(&app).map(|m| m.severity),
        Some(Severity::Error)
    );

    let msg = cmd.run().expect("Trash Cmd replies with a Msg");
    send(&mut app, msg);

    assert!(
        !app.documents.contains_key(&closing_id),
        "the first request's reply must still land normally"
    );
    let text = messages::log_text(&app);
    assert!(text.contains("moved to Trash"), "log must say so: {text:?}");
}

/// `Msg::TrashDone{Ok}` closes the open tab — minting a fresh Untitled
/// since it was the last document — and posts "moved to Trash".
#[test]
fn trash_done_ok_closes_the_tab_and_posts_moved() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let closing_id = app.active;
    send(&mut app, sup_backspace());
    let mut effects = send(&mut app, yes());
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("Trash Cmd replies with a Msg");

    send(&mut app, msg);

    assert!(!app.documents.contains_key(&closing_id));
    assert_eq!(
        app.documents.len(),
        1,
        "a fresh Untitled replaces the last doc"
    );
    assert!(
        mem.read(Path::new("/root/a.md")).is_err(),
        "trash actually removed the file from Mem"
    );
    let text = messages::log_text(&app);
    assert!(text.contains("moved to Trash"), "log must say so: {text:?}");
}

/// A4: the document became dirty between confirm and the reply landing —
/// the tab stays open, content is intact, and a Warn message is posted.
#[test]
fn dirty_at_reply_keeps_the_tab_open_with_a_warning() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let id = app.active;
    send(&mut app, sup_backspace());
    let mut effects = send(&mut app, yes());
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("Trash Cmd replies with a Msg");

    dirty_common::force_dirty(&mut app, id);
    send(&mut app, msg);

    assert!(
        app.documents.contains_key(&id),
        "a dirty document must not be closed out from under the user"
    );
    assert_eq!(
        app.doc(id).map(|d| d.buffer.content().to_string()),
        Some("!a content".to_string())
    );
    let text = messages::log_text(&app);
    assert!(
        text.contains("unsaved changes kept in the open tab"),
        "the dirty-at-reply Warn message must be posted: {text:?}"
    );
}

/// A `DirtyClose` guard raised for the SAME document while the trash `Cmd`
/// is in flight is cleared before the close lands — the footer must not go
/// on rendering a prompt for a document that no longer exists.
#[test]
fn guard_at_reply_is_cleared_before_the_close() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let id = app.active;
    send(&mut app, sup_backspace());
    let mut effects = send(&mut app, yes());
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("Trash Cmd replies with a Msg");

    // A second document opened+closed while the Cmd was in flight raised a
    // DirtyClose guard for the FIRST (now-being-trashed) document — an
    // artificial but faithful stand-in for "some other in-flight guard
    // targets the same doc" (plan Gotchas).
    app.guard = Some(rune_tui::guard::GuardPrompt {
        doc: id,
        kind: GuardKind::DirtyClose,
    });

    send(&mut app, msg);

    assert!(
        app.guard.is_none(),
        "the stale guard for the closed document must be swept"
    );
    assert!(
        !app.documents.contains_key(&id),
        "the trashed document must actually be closed"
    );
    let text = rune_tui::footer::footer_text(&app);
    assert!(
        !text.contains("[S]ave"),
        "footer must not keep prompting for a trashed document: {text:?}"
    );
}

/// `Msg::TrashDone{Err}` posts the error and closes nothing.
#[test]
fn trash_done_err_posts_and_closes_nothing() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let id = app.active;
    send(&mut app, sup_backspace());
    let mut effects = send(&mut app, yes());
    let cmd = effects.cmds.remove(0);
    let CmdKind::Trash = cmd.kind() else {
        panic!("expected a Trash Cmd");
    };
    drop(cmd);

    let generation = 1;
    send(
        &mut app,
        Msg::TrashDone {
            generation,
            path: PathBuf::from("/root/a.md"),
            result: Err("disk full".to_string()),
        },
    );

    assert!(app.documents.contains_key(&id));
    let text = messages::log_text(&app);
    assert!(
        text.contains("disk full"),
        "error text must surface: {text:?}"
    );
}

/// A stale-generation `TrashDone` (an earlier request's reply landing after
/// a fresher one superseded it) is dropped on arrival.
#[test]
fn stale_generation_trash_done_is_ignored() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let id = app.active;
    send(&mut app, sup_backspace());
    send(&mut app, yes()); // mints generation 1, app.trash_gen == 1

    send(
        &mut app,
        Msg::TrashDone {
            generation: 0,
            path: PathBuf::from("/root/a.md"),
            result: Ok(()),
        },
    );

    assert!(
        app.documents.contains_key(&id),
        "a stale reply must never close the document"
    );
    assert!(
        mem.read(Path::new("/root/a.md")).is_ok(),
        "a stale reply must not be treated as a real trash"
    );
}

/// Path-equality pin: a document opened under a spelling different from
/// the trashed path is NOT recognized as the same document — the
/// inherited exact-`PathBuf`-equality limitation this module documents.
#[test]
fn path_equality_is_exact_not_resolved() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let id = app.active;
    // Same real file on a case-insensitive filesystem, spelled with a
    // different case — `existing_document_for` compares raw `PathBuf`s
    // (case-sensitive `Eq`), so this spelling never matches `/root/a.md`
    // even though the OS would treat them as the same file.
    let differently_spelled = PathBuf::from("/ROOT/a.md");

    send(&mut app, sup_backspace());
    let mut effects = send(&mut app, yes());
    let cmd = effects.cmds.remove(0);
    let generation = 1;
    drop(cmd);

    send(
        &mut app,
        Msg::TrashDone {
            generation,
            path: differently_spelled,
            result: Ok(()),
        },
    );

    assert!(
        app.documents.contains_key(&id),
        "an exact-match miss must leave the open document alone"
    );
}
