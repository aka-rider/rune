//! Rename "Done when" tests: the collision guard and both halves of
//! hazard 1 (a prompt that is never raised, and one that is displaced
//! later), the stale-ticket drop, and the close-while-colliding cleanup —
//! TODO.md's 500-line budget split of the original `rename.rs`. Focus/typing, the
//! refusals, the no-store end-to-end rename, and draft naming live in the
//! sibling `rename_bind.rs`; the store-backed `[R]eplace` path lives in
//! `rename_replace.rs`; the WP2 focus-loss-is-the-commit-chokepoint suite
//! lives in `rename_focus.rs`. All four pull shared fixtures from
//! `rename_common`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;

use rune_tui::guard::{self, GuardKind};
use rune_tui::keymap::KeyCode;
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::runtime::{CmdKind, Effects};
use rune_tui::{footer, messages, workspace};

use rune_vfs::Vfs;

use rename_common::{app_with, collide, plain, rename_to, seeded_vfs, send};

/// A collision with no Guard up raises the guard, enters `Collision`, and
/// the footer names the target.
#[test]
fn a_collision_raises_the_guard_and_the_footer_names_the_target() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);
    send(&mut app, reply);

    assert!(matches!(app.rename, RenameState::Collision { .. }));
    assert!(matches!(
        app.guard,
        Some(ref p) if matches!(p.kind, GuardKind::RenameCollision { .. })
    ));
    let text = footer::footer_text(&app);
    assert!(
        text.contains("b.md"),
        "footer must name the target: {text:?}"
    );
    assert!(text.contains(guard::GUARD_CANCEL.help));

    // Both files are still intact — a collision writes nothing.
    assert_eq!(mem.read(Path::new("/root/a.md")).unwrap(), b"a content");
    assert_eq!(mem.read(Path::new("/root/b.md")).unwrap(), b"theirs");
}

/// **Hazard 1a**: errors are a non-modal log entry now, so a pending
/// message can no longer suppress a Guard raise the way the old modal
/// error banner used to — the collision guard must still raise, and the
/// earlier message must still be in the log.
#[test]
fn a_pending_error_message_does_not_suppress_the_collision_guard() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);

    messages::error(&mut app, "something else went wrong");
    send(&mut app, reply);

    assert!(
        matches!(app.rename, RenameState::Collision { .. }),
        "an error message must never suppress a genuine Guard raise"
    );
    assert!(matches!(
        app.guard,
        Some(ref p) if matches!(p.kind, GuardKind::RenameCollision { .. })
    ));
    assert_eq!(
        messages::newest_text(&app),
        Some("something else went wrong"),
        "the earlier message must still be in the log"
    );
}

/// **Hazard 1b**: an error posted AFTER the collision guard
/// is up must not cancel it — errors and the Guard are orthogonal channels
/// now, unlike the old modal error banner that used to
/// displace it.
#[test]
fn an_error_message_posted_after_the_guard_does_not_cancel_the_collision() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);
    send(&mut app, reply);
    assert!(matches!(app.rename, RenameState::Collision { .. }));

    messages::error(&mut app, "boom");

    assert!(
        matches!(app.rename, RenameState::Collision { .. }),
        "an unrelated error message must not cancel an in-progress collision guard"
    );
    assert!(app.guard.is_some());
    assert_eq!(messages::newest_text(&app), Some("boom"));
}

/// `Esc` on the guard clears it, returns to `Idle`, and leaves the field
/// holding the typed name (not the old committed one).
#[test]
fn escape_on_the_collision_guard_returns_to_the_title_with_the_typed_name() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);
    send(&mut app, reply);

    send(&mut app, plain(KeyCode::Escape));

    assert_eq!(app.rename, RenameState::Idle);
    assert!(app.guard.is_none());
    assert_eq!(app.focus(), Pane::Title);
    assert_eq!(app.title.text(), "b.md");
}

/// Escape used to leave the user with no feedback at all — the modal just
/// vanished. Pin that cancelling the rename-collision Guard now names what
/// it cancelled via a status message.
#[test]
fn escape_on_the_rename_collision_guard_sets_a_cancellation_status() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);
    send(&mut app, reply);

    send(&mut app, plain(KeyCode::Escape));

    assert_eq!(messages::newest_text(&app), Some("rename cancelled"));
}

/// `r` on the guard for a `db: None` document cannot capture the displaced
/// bytes, so the prompt stays up with an explanation and the disk
/// is untouched. The footer must not offer `[R]eplace` either.
#[test]
fn replace_is_refused_and_unoffered_without_a_store() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);
    send(&mut app, reply);

    assert!(
        !footer::footer_text(&app).contains(guard::RENAME_REPLACE.help),
        "an option the app would refuse must not be offered"
    );

    send(&mut app, plain(KeyCode::Char('r')));

    assert!(matches!(app.rename, RenameState::Collision { .. }));
    assert!(app.guard.is_some(), "the prompt must stay up");
    assert!(
        messages::newest_text(&app).is_some_and(|m| m.contains("cannot replace")),
        "got {:?}",
        messages::newest_text(&app)
    );
    assert_eq!(mem.read(Path::new("/root/a.md")).unwrap(), b"a content");
    assert_eq!(mem.read(Path::new("/root/b.md")).unwrap(), b"theirs");
}

/// A stale-generation `Msg::RenameDone` (dismissed, then restarted) must
/// leave the fresh state alone.
#[test]
fn a_stale_rename_reply_is_dropped() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    let mut first = rename_to(&mut app, "b");
    let stale = first
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");

    // Abandon it and start a fresh one under a new generation.
    app.rename = RenameState::Idle;
    let mut second = rename_to(&mut app, "c");
    assert!(matches!(app.rename, RenameState::Committing { .. }));
    let fresh_state = app.rename.clone();
    let _fresh_cmd = second
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");

    // The FIRST cmd's reply lands late.
    send(&mut app, stale.run().expect("a reply"));

    assert_eq!(
        app.rename, fresh_state,
        "a stale reply must not disturb the fresh rename"
    );
}

/// `close_now` on the renaming document while `Collision` clears both the
/// machine and its prompt.
#[test]
fn closing_the_renaming_document_clears_the_machine_and_the_prompt() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/c.md"), b"c content")
        .expect("seed c.md");
    let mut app = app_with(&mem);
    // A second document, so the last-document floor doesn't refuse.
    workspace::open_path(&mut app, Path::new("/root/c.md"));
    let first_tab = app.documents.order()[0];
    workspace::switch_to(&mut app, first_tab);
    let victim = app.active;

    let reply = collide(&mut app, &mem);
    send(&mut app, reply);
    assert!(matches!(app.rename, RenameState::Collision { .. }));

    let _ = workspace::close_now(&mut app, victim, &mut Effects::default());

    assert_eq!(app.rename, RenameState::Idle);
    assert!(app.guard.is_none());
}
