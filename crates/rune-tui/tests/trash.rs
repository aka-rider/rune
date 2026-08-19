//! Tests for `⌘⌫`/`^⌫` trash: the guard raise/cancel,
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
mod explorer_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_fuzz::Session;
use rune_tui::app;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::messages;
use rune_tui::messages::Severity;
use rune_tui::pane::Pane;
use rune_tui::runtime::{CmdError, CmdKind, Effects, Msg};

use rune_vfs::{Mem, Vfs};

/// [`explorer_common::open_seeded`], with the store stripped back out
/// (`App::db` back to `None`) — the trash flow always takes the no-store
/// `Cmd` route (plan: rune-db gets no purge path), so a document must stay
/// unbound for `Msg::SaveDone`'s no-store fallback (Assumption A1) to stay
/// reachable through this suite.
fn app_with(mem: &Arc<Mem>) -> Session {
    let mut session = explorer_common::open_seeded(mem);
    if let Some(db) = session.app_mut().db.take() {
        db.shutdown();
    }
    session
}

fn select_row(session: &mut Session, name: &str) {
    let idx = session
        .app()
        .explorer
        .entries
        .iter()
        .position(|e| e.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"));
    session.app_mut().explorer.nav.cursor = idx;
}

fn send(session: &mut Session, msg: Msg) -> Effects {
    let mut effects = Effects::default();
    app::update(session.app_mut(), msg, &mut effects);
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

/// `⌘⌫` on a clean, named document raises the Trash guard and the footer
/// shows the confirm prompt naming the file.
#[test]
fn clean_named_doc_raises_the_guard_and_names_the_file() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    send(&mut session, sup_backspace());

    assert!(matches!(
        session.app().guard,
        Some(ref p) if matches!(p.kind, GuardKind::Trash { ref path, .. } if path == Path::new("/root/a.md"))
    ));
    let text = rune_tui::footer::footer_text(session.app());
    assert!(text.contains("a.md"), "footer must name the file: {text:?}");
    assert!(
        text.contains("Y yes"),
        "footer must offer the answer: {text:?}"
    );
}

/// `Esc` cancels the trash guard with "trash cancelled".
#[test]
fn escape_cancels_with_trash_cancelled() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    send(&mut session, sup_backspace());
    send(&mut session, escape());

    assert!(session.app().guard.is_none());
    assert_eq!(
        messages::newest_text(session.app()),
        Some("trash cancelled")
    );
}

/// A dirty document refuses the trash outright — no guard raised, an error
/// posted instead.
#[test]
fn dirty_doc_is_refused_with_no_guard() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    let id = session.app().active;
    dirty_common::force_dirty(session.app_mut(), id);

    send(&mut session, sup_backspace());

    assert!(session.app().guard.is_none());
    assert_eq!(
        messages::newest(session.app()).map(|m| m.severity),
        Some(Severity::Error)
    );
}

/// A pathless draft has nothing to trash.
#[test]
fn pathless_draft_is_refused() {
    let mut session = Session::open("draft.md", "draft");
    session.app_mut().active_doc_mut().file_path = None;

    send(&mut session, sup_backspace());

    assert!(session.app().guard.is_none());
    assert_eq!(
        messages::newest(session.app()).map(|m| m.severity),
        Some(Severity::Error)
    );
}

/// Explorer focus with a directory selected is refused.
#[test]
fn explorer_directory_selection_is_refused() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    explorer_common::drive_load_explorer(&mut session);
    assert_eq!(session.app().focus(), Pane::Explorer);
    let idx = session
        .app()
        .explorer
        .entries
        .iter()
        .position(|e| e.kind == rune_vfs::FileKind::Dir)
        .expect("sub is listed");
    session.app_mut().explorer.nav.cursor = idx;

    send(&mut session, sup_backspace());

    assert!(session.app().guard.is_none());
    assert_eq!(
        messages::newest(session.app()).map(|m| m.severity),
        Some(Severity::Error)
    );
}

/// Explorer focus with a file selected carries THAT file's path, not the
/// active document's.
#[test]
fn explorer_file_selection_carries_that_files_path() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    explorer_common::drive_load_explorer(&mut session);
    let idx = session
        .app()
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "b.md")
        .expect("b.md is listed");
    session.app_mut().explorer.nav.cursor = idx;

    send(&mut session, sup_backspace());

    assert!(matches!(
        session.app().guard,
        Some(ref p) if matches!(p.kind, GuardKind::Trash { ref path, .. } if path == Path::new("/root/b.md"))
    ));
}

/// Trashing a symlink row removes the LINK and leaves the document it
/// points at untouched — the row's literal path reaches `Vfs::trash`, never
/// a resolved one.
#[test]
fn trashing_a_symlink_removes_the_link_and_leaves_its_target_readable() {
    let mem = explorer_common::seeded_vfs();
    mem.symlink(Path::new("/root/link.md"), Path::new("/root/b.md"))
        .expect("seed a symlink to b.md");
    let mut session = app_with(&mem);
    explorer_common::drive_load_explorer(&mut session);
    select_row(&mut session, "link.md");

    send(&mut session, sup_backspace());
    assert!(matches!(
        session.app().guard,
        Some(ref p) if matches!(p.kind, GuardKind::Trash { ref path, .. } if path == Path::new("/root/link.md"))
    ));
    let mut effects = send(&mut session, yes());
    let msg = effects.cmds.remove(0).run().expect("Trash Cmd replies");
    send(&mut session, msg);

    assert_eq!(
        mem.read(Path::new("/root/b.md")).expect("target survives"),
        b"b content".to_vec()
    );
    assert!(
        mem.read(Path::new("/root/link.md")).is_err(),
        "the link itself is gone"
    );
}

/// A symlink to a directory is trashable — the link is what goes — and the
/// confirmation says so rather than describing it as a directory.
#[test]
fn the_confirmation_for_a_symlinked_directory_says_symlink() {
    let mem = explorer_common::seeded_vfs();
    mem.symlink(Path::new("/root/subalias"), Path::new("/root/sub"))
        .expect("seed a symlink to sub");
    let mut session = app_with(&mem);
    explorer_common::drive_load_explorer(&mut session);
    select_row(&mut session, "subalias");

    send(&mut session, sup_backspace());

    assert!(matches!(
        session.app().guard,
        Some(ref p) if matches!(p.kind, GuardKind::Trash { ref path, .. } if path == Path::new("/root/subalias"))
    ));
    let text = rune_tui::footer::footer_text(session.app());
    assert!(
        text.contains("Trash symlink subalias?"),
        "the prompt must name what is actually removed: {text:?}"
    );
}

/// A real directory is still refused outright.
#[test]
fn a_real_directory_is_still_refused() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    explorer_common::drive_load_explorer(&mut session);
    select_row(&mut session, "sub");

    send(&mut session, sup_backspace());

    assert!(session.app().guard.is_none());
    assert_eq!(
        messages::newest_text(session.app()),
        Some("cannot trash a directory")
    );
}

/// `y` enqueues exactly one `Trash` `Cmd`.
#[test]
fn yes_enqueues_a_trash_cmd() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    send(&mut session, sup_backspace());
    let effects = send(&mut session, yes());

    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::Trash);
    assert!(session.app().guard.is_none());
}

/// A second `⌘⌫`+`y` on the same still-clean doc while the first trash
/// `Cmd` is in flight is refused (single-flight): no second `Cmd` is
/// enqueued, no guard is raised, an error is posted — and the first
/// request's reply still lands normally once it arrives.
#[test]
fn second_trash_while_one_in_flight_is_refused() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    let closing_id = session.app().active;
    send(&mut session, sup_backspace());
    let mut effects = send(&mut session, yes());
    assert_eq!(effects.cmds.len(), 1);
    let cmd = effects.cmds.remove(0);

    send(&mut session, sup_backspace());

    assert!(
        session.app().guard.is_none(),
        "a second trash request must not raise a guard while one is in flight"
    );
    assert_eq!(
        messages::newest(session.app()).map(|m| m.severity),
        Some(Severity::Error)
    );

    let msg = cmd.run().expect("Trash Cmd replies with a Msg");
    send(&mut session, msg);

    assert!(
        !session.app().documents.contains_key(&closing_id),
        "the first request's reply must still land normally"
    );
    let text = messages::log_text(session.app());
    assert!(text.contains("moved to Trash"), "log must say so: {text:?}");
}

/// `Msg::TrashDone{Ok}` closes the open tab — minting a fresh Untitled
/// since it was the last document — and posts "moved to Trash".
#[test]
fn trash_done_ok_closes_the_tab_and_posts_moved() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    let closing_id = session.app().active;
    send(&mut session, sup_backspace());
    let mut effects = send(&mut session, yes());
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("Trash Cmd replies with a Msg");

    send(&mut session, msg);

    assert!(!session.app().documents.contains_key(&closing_id));
    assert_eq!(
        session.app().documents.len(),
        1,
        "a fresh Untitled replaces the last doc"
    );
    assert!(
        mem.read(Path::new("/root/a.md")).is_err(),
        "trash actually removed the file from Mem"
    );
    let text = messages::log_text(session.app());
    assert!(text.contains("moved to Trash"), "log must say so: {text:?}");
}

/// A4: the document became dirty between confirm and the reply landing —
/// the tab stays open, content is intact, and a Warn message is posted.
#[test]
fn dirty_at_reply_keeps_the_tab_open_with_a_warning() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    let id = session.app().active;
    send(&mut session, sup_backspace());
    let mut effects = send(&mut session, yes());
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("Trash Cmd replies with a Msg");

    dirty_common::force_dirty(session.app_mut(), id);
    send(&mut session, msg);

    assert!(
        session.app().documents.contains_key(&id),
        "a dirty document must not be closed out from under the user"
    );
    assert_eq!(
        session
            .app()
            .doc(id)
            .map(|d| d.buffer.content().to_string()),
        Some("!a content".to_string())
    );
    let text = messages::log_text(session.app());
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
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    let id = session.app().active;
    send(&mut session, sup_backspace());
    let mut effects = send(&mut session, yes());
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("Trash Cmd replies with a Msg");

    // A second document opened+closed while the Cmd was in flight raised a
    // DirtyClose guard for the FIRST (now-being-trashed) document — an
    // artificial but faithful stand-in for "some other in-flight guard
    // targets the same doc" (plan Gotchas). No production path opens and
    // dirty-closes a SECOND document while a trash Cmd for the first is
    // still in flight without also resolving or superseding that second
    // guard first, so this stays a direct fixture poke rather than a real
    // two-guard race driven through public input.
    session.app_mut().guard = Some(rune_tui::guard::GuardPrompt {
        doc: id,
        kind: GuardKind::DirtyClose,
    });

    send(&mut session, msg);

    assert!(
        session.app().guard.is_none(),
        "the stale guard for the closed document must be swept"
    );
    assert!(
        !session.app().documents.contains_key(&id),
        "the trashed document must actually be closed"
    );
    let text = rune_tui::footer::footer_text(session.app());
    assert!(
        !text.contains("Y yes"),
        "footer must not keep prompting for a trashed document: {text:?}"
    );
}

/// `Msg::TrashDone{Err}` posts the error and closes nothing.
#[test]
fn trash_done_err_posts_and_closes_nothing() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    let id = session.app().active;
    send(&mut session, sup_backspace());
    let mut effects = send(&mut session, yes());
    let cmd = effects.cmds.remove(0);
    let CmdKind::Trash = cmd.kind() else {
        panic!("expected a Trash Cmd");
    };
    drop(cmd);

    let generation = rune_tui::generation::Generation::ZERO; // the first (and only) mint
    send(
        &mut session,
        Msg::TrashDone {
            generation,
            path: PathBuf::from("/root/a.md"),
            result: Err(CmdError::Refused("disk full".to_string())),
        },
    );

    assert!(session.app().documents.contains_key(&id));
    let text = messages::log_text(session.app());
    assert!(
        text.contains("disk full"),
        "error text must surface: {text:?}"
    );
}

/// A stale-generation `TrashDone` (an earlier request's reply landing after
/// a fresher one superseded it) is dropped on arrival.
#[test]
fn stale_generation_trash_done_is_ignored() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    let id = session.app().active;
    send(&mut session, sup_backspace());
    send(&mut session, yes()); // mints generation 0, app.trash_gen == 0

    send(
        &mut session,
        Msg::TrashDone {
            generation: rune_tui::generation::Generation::from_raw(1), // never minted — stale
            path: PathBuf::from("/root/a.md"),
            result: Ok(()),
        },
    );

    assert!(
        session.app().documents.contains_key(&id),
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
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    let id = session.app().active;
    // Same real file on a case-insensitive filesystem, spelled with a
    // different case — `existing_document_for` compares raw `PathBuf`s
    // (case-sensitive `Eq`), so this spelling never matches `/root/a.md`
    // even though the OS would treat them as the same file.
    let differently_spelled = PathBuf::from("/ROOT/a.md");

    send(&mut session, sup_backspace());
    let mut effects = send(&mut session, yes());
    let cmd = effects.cmds.remove(0);
    let generation = rune_tui::generation::Generation::ZERO; // the first (and only) mint
    drop(cmd);

    send(
        &mut session,
        Msg::TrashDone {
            generation,
            path: differently_spelled,
            result: Ok(()),
        },
    );

    assert!(
        session.app().documents.contains_key(&id),
        "an exact-match miss must leave the open document alone"
    );
}
