//! Tests for `⌘⌫`/`^⌫` trash: the guard raise/cancel, every refusal
//! (dirty, pathless, directory), and the confirm's `Cmd` enqueue. This is
//! the 500-line-budget split of the original `trash.rs`: `Msg::TrashDone`'s
//! close/keep-open/error/stale-generation reply branches (including the
//! async A4 dirty-at-reply and guard-at-reply cases) and the inherited
//! exact-path-match limitation live in the sibling `trash_reply.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod dirty_common;
mod explorer_common;
mod trash_common;

use std::path::Path;

use rune_tui::guard::GuardKind;
use rune_tui::messages;
use rune_tui::messages::Severity;
use rune_tui::pane::Pane;
use rune_tui::runtime::CmdKind;

use rune_fuzz::Session;
use rune_vfs::Vfs;

use trash_common::{app_with, escape, select_row, send, sup_backspace, yes};

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
