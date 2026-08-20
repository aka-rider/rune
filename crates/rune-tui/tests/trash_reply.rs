//! Tests for `Msg::TrashDone`'s close/keep-open/error/stale-generation
//! branches (including the async A4 dirty-at-reply and guard-at-reply
//! cases), and the inherited exact-path-match limitation. This is the
//! 500-line-budget split of the original `trash.rs`: the guard raise/
//! cancel, every refusal (dirty, pathless, directory), and the confirm's
//! `Cmd` enqueue live in the sibling `trash.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod dirty_common;
mod explorer_common;
mod trash_common;

use std::path::{Path, PathBuf};

use rune_tui::guard::GuardKind;
use rune_tui::messages;
use rune_tui::runtime::{CmdError, CmdKind, Msg};

use rune_vfs::Vfs;

use trash_common::{app_with, send, sup_backspace, yes};

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
