//! Tests for Trash's guard raise/cancel, every refusal (dirty, pathless,
//! directory, in-flight rename), and the confirm's `Cmd` enqueue. Trash is
//! reachable two ways now (product decision, `trash_scope.rs`'s own module
//! doc has the full story): an Explorer-pane-scoped `⌘⌫`/Delete, driven
//! through `explorer_common::drive_load_explorer` below, and the command
//! palette's "trash" row, driven through `trash_common::trash_via_palette`
//! for the tests that target the active document without Explorer focus.
//! This is the 500-line-budget split of the original `trash.rs`:
//! `Msg::TrashDone`'s close/keep-open/error/stale-generation reply branches
//! (including the async A4 dirty-at-reply and guard-at-reply cases) and the
//! inherited exact-path-match limitation live in the sibling
//! `trash_reply.rs`; the no-longer-global chord and overlay-scoping tests
//! live in `trash_scope.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod dirty_common;
mod explorer_common;
mod rename_common;
mod trash_common;

use std::path::Path;

use rune_tui::guard::GuardKind;
use rune_tui::messages;
use rune_tui::messages::Severity;
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::runtime::CmdKind;

use rune_fuzz::Session;
use rune_vfs::Vfs;

use trash_common::{app_with, escape, select_row, send, sup_backspace, trash_via_palette, yes};

/// The palette's "trash" row on a clean, named document raises the Trash
/// guard and the footer shows the confirm prompt naming the file — the
/// active document, reached without any Explorer focus.
#[test]
fn clean_named_doc_raises_the_guard_and_names_the_file() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    trash_via_palette(&mut session);

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
    trash_via_palette(&mut session);
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

    trash_via_palette(&mut session);

    assert!(session.app().guard.is_none());
    assert_eq!(
        messages::newest(session.app()).map(|m| m.severity),
        Some(Severity::Error)
    );
}

/// A pathless draft, with no Explorer selection to fall back to, has
/// nothing to trash — the palette row itself is Unavailable, so Enter never
/// even reaches `trash::request_trash`.
#[test]
fn pathless_draft_is_refused_via_the_palette() {
    let mut session = Session::open("draft.md", "draft");
    session.app_mut().active_doc_mut().file_path = None;

    trash_via_palette(&mut session);

    assert!(session.app().guard.is_none());
    assert_eq!(
        session.app().palette().and_then(|s| s.refusal.clone()),
        Some("nothing to trash \u{2014} draft has no file".to_string())
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
    trash_via_palette(&mut session);
    let effects = send(&mut session, yes());

    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::Trash);
    assert!(session.app().guard.is_none());
}

/// A second trash request on the same still-clean doc while the first trash
/// `Cmd` is in flight is refused (single-flight): no second `Cmd` is
/// enqueued, no guard is raised, an error is posted — and the first
/// request's reply still lands normally once it arrives.
#[test]
fn second_trash_while_one_in_flight_is_refused() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    let closing_id = session.app().active;
    trash_via_palette(&mut session);
    let mut effects = send(&mut session, yes());
    assert_eq!(effects.cmds.len(), 1);
    let cmd = effects.cmds.remove(0);

    trash_via_palette(&mut session);

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

/// Defect 2: a rename enqueued (ack still pending) races a trash attempt on
/// the very same file — `rename_excl(old\u{2192}new)` and a `Trash` `Cmd`
/// reading the still-old path would otherwise both act on one inode. Trash
/// refuses outright, with feedback, and enqueues nothing; the in-flight
/// rename itself is left untouched and completes normally once its own ack
/// lands.
#[test]
fn trash_refuses_while_a_rename_is_in_flight() {
    let mem = explorer_common::seeded_vfs();
    let mut session = rename_common::store_session(&mem, "/root/a.md");
    rename_common::commit_name(&mut session, "renamed");
    assert!(
        matches!(session.app().rename, RenameState::Committing { .. }),
        "test setup: the rename's own ack must still be pending"
    );

    trash_via_palette(&mut session);

    assert!(
        session.app().guard.is_none(),
        "no trash guard may be raised while a rename races it"
    );
    assert_eq!(
        messages::newest_text(session.app()),
        Some("can't trash while a rename is in flight")
    );
    assert!(
        matches!(session.app().rename, RenameState::Committing { .. }),
        "the refused trash attempt must not disturb the in-flight rename"
    );

    assert!(session.deliver_db_all().is_none());
    assert_eq!(
        session.app().rename,
        RenameState::Idle,
        "the rename itself completes normally once its ack lands"
    );
}
