//! Rename "Done when" test: the `[R]eplace` path against a real in-memory
//! `Store` — TODO.md's 500-line budget split of the original `rename.rs`. Focus/
//! typing, the refusals, the no-store end-to-end rename, and draft naming
//! live in the sibling `rename_bind.rs`; the collision guard and hazard-1
//! tests live in `rename_collision.rs`; the WP2 focus-loss-is-the-commit-
//! chokepoint suite lives in `rename_focus.rs`. All four pull shared
//! fixtures from `rename_common`.

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
use rune_tui::runtime::Msg;

use rune_vfs::Vfs;

use rename_common::{active_path, app_with_store, next_event, plain, rename_to, send};

/// The full `[R]eplace`: `r` enqueues an `OpKind::RenameReplace`, and the
/// ack binds the new path and reports the preserved bytes.
#[test]
fn replace_with_a_real_store_preserves_the_displaced_bytes() {
    let mem = rename_common::seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"theirs")
        .expect("seed b.md");
    let (mut app, rx) = app_with_store(&mem);

    // Drive the collision through the store route.
    rename_to(&mut app, "b");
    assert!(matches!(app.rename, RenameState::Committing { .. }));
    let evt = next_event(&rx);
    send(&mut app, Msg::Db(evt));

    assert!(
        matches!(app.rename, RenameState::Collision { .. }),
        "expected a collision, got {:?}",
        app.rename
    );
    assert!(
        footer::footer_text(&app).contains(guard::RENAME_REPLACE.help),
        "a store-bound document must be offered the replace answer"
    );

    let ops_before = app.db_ops.len();
    send(&mut app, plain(KeyCode::Char('r')));
    assert!(
        matches!(app.rename, RenameState::Capturing { .. }),
        "expected Capturing, got {:?}",
        app.rename
    );
    assert_eq!(app.db_ops.len(), ops_before + 1, "one replace op enqueued");
    assert!(app.guard.is_none(), "the prompt is resolved");

    let evt = next_event(&rx);
    send(&mut app, Msg::Db(evt));

    assert_eq!(app.rename, RenameState::Idle);
    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/b.md")));
    assert_eq!(mem.read(Path::new("/root/b.md")).unwrap(), b"a content");
    assert!(mem.read(Path::new("/root/a.md")).is_err());
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|m| m.contains("preserved")),
        "the status must say the replaced bytes were kept, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
}
