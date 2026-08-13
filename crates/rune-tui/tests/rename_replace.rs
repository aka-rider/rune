//! Rename "Done when" test: the `[R]eplace` path against a real in-memory
//! `Store`, driven through `rune_fuzz::Session` — TODO.md's 500-line
//! budget split of the original `rename.rs`. Focus/typing, the end-to-end
//! `Cmd`-route rename, and draft naming live in the sibling
//! `rename_bind.rs`; the refusals in `rename_refusals.rs`; the collision
//! guard and hazard-1 tests in `rename_collision.rs`; the
//! focus-loss-is-the-commit-chokepoint suite in `rename_focus.rs`. All
//! pull shared fixtures from `rename_common`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;

use rune_tui::footer;
use rune_tui::guard;
use rune_tui::keymap::KeyCode;
use rune_tui::rename::RenameState;
use rune_vfs::Vfs;

use rename_common::{active_path, bound_session, commit_name, plain_key};

/// The full `[R]eplace`: `r` enqueues an `OpKind::RenameReplace`, and the
/// ack binds the new path and reports the preserved bytes.
#[test]
fn replace_with_a_real_store_preserves_the_displaced_bytes() {
    let (mut session, mem) = bound_session();
    mem.save_atomic(Path::new("/root/b.md"), b"theirs")
        .expect("seed b.md");

    // Drive the collision through the store route.
    commit_name(&mut session, "b");
    assert!(matches!(
        session.app().rename,
        RenameState::Committing { .. }
    ));
    assert!(session.deliver_db().is_none());

    assert!(
        matches!(session.app().rename, RenameState::Collision { .. }),
        "expected a collision, got {:?}",
        session.app().rename
    );
    assert!(
        footer::footer_text(session.app()).contains(guard::RENAME_REPLACE.help),
        "a store-bound document must be offered the replace answer"
    );

    let ops_before = session.app().db_ops.len();
    assert!(session.key(plain_key(KeyCode::Char('r'))).is_none());
    assert!(
        matches!(session.app().rename, RenameState::Capturing { .. }),
        "expected Capturing, got {:?}",
        session.app().rename
    );
    assert_eq!(
        session.app().db_ops.len(),
        ops_before + 1,
        "one replace op enqueued"
    );
    assert!(session.app().guard.is_none(), "the prompt is resolved");

    assert!(session.deliver_db().is_none());

    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(
        active_path(session.app()).as_deref(),
        Some(Path::new("/root/b.md"))
    );
    assert_eq!(mem.read(Path::new("/root/b.md")).unwrap(), b"a content");
    assert!(mem.read(Path::new("/root/a.md")).is_err());
    assert!(
        rune_tui::messages::newest_text(session.app()).is_some_and(|m| m.contains("preserved")),
        "the status must say the replaced bytes were kept, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
}
