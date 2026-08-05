//! Rename "Done when" tests: focus/typing, the end-to-end no-store
//! rename, and draft naming — TODO.md's §1.6 split of the original
//! `rename.rs`, itself re-split by plan WP5 once the extension-gate and
//! clipboard packages grew this file past the ceiling again: the refusal
//! paths now live in `rename_refusals.rs`, the extension gate and the
//! field's own word-motion/selection/undo editing in `rename_gate.rs`,
//! and copy/cut/paste in `rename_clipboard.rs`. The collision guard/
//! hazard-1 tests, the store-backed `[R]eplace` path, and the WP2
//! focus-loss-is-the-commit-chokepoint suite live in the further siblings
//! `rename_collision.rs`/`rename_replace.rs`/`rename_focus.rs`; all seven
//! pull shared fixtures from `rename_common`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod dirty_common;
mod rename_common;

use std::path::Path;
use std::sync::Arc;

use rune_tui::app::App;
use rune_tui::keymap::KeyCode;
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::runtime::CmdKind;

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, Vfs};

use rename_common::{active_path, app_with, ctrl, plain, rename_to, seeded_vfs, send, type_text};

// ── Focus and typing ────────────────────────────────────────────────────

/// `^r` focuses the title, seeded with the file's FULL NAME (extension
/// included), and typing there never touches the buffer — the
/// `PANE-NO-BLEED` property, asserted directly.
#[test]
fn ctrl_r_focuses_the_title_and_typing_never_touches_the_buffer() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));
    assert_eq!(app.focus(), Pane::Title);
    assert_eq!(
        app.title.text(),
        "a.md",
        "seeded with the full name, extension included"
    );

    type_text(&mut app, "xyz");
    assert_eq!(app.title.text(), "axyz.md");
    assert_eq!(
        app.active_doc().buffer.content(),
        before,
        "a keystroke aimed at the file name must never reach the buffer"
    );
}

/// `Esc` reverts to the committed name and refocuses the editor without
/// renaming anything.
#[test]
fn escape_reverts_the_field_and_renames_nothing() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    type_text(&mut app, "zzz");
    send(&mut app, plain(KeyCode::Escape));

    assert_eq!(app.title.text(), "a.md");
    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/a.md")));
    assert!(mem.read(Path::new("/root/a.md")).is_ok());
}

/// Up at the top of the buffer focuses the title (a contextual gesture, no
/// new binding).
#[test]
fn up_at_the_top_of_the_editor_focuses_the_title() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    send(&mut app, plain(KeyCode::Up));
    assert_eq!(app.focus(), Pane::Title);
}

// ── End to end, no store ────────────────────────────────────────────────

/// One `CmdKind::Rename`, run it, feed the reply: the file moves, the tab
/// and title show the new name, and `is_dirty()` is UNCHANGED — a rename
/// is not a save (§1.4.2).
#[test]
fn end_to_end_no_store_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    // Make the document dirty, so "stays dirty" is actually observable.
    let active = app.active;
    dirty_common::force_dirty(&mut app, active);
    let dirty_before = app.is_dirty();
    assert!(dirty_before, "test setup: the document must be dirty");

    let mut effects = rename_to(&mut app, "b");
    let cmds: Vec<_> = effects
        .cmds
        .drain(..)
        .filter(|c| c.kind() == CmdKind::Rename)
        .collect();
    assert_eq!(cmds.len(), 1, "exactly one Rename Cmd");
    assert!(matches!(app.rename, RenameState::Committing { .. }));

    let msg = cmds.into_iter().next().unwrap().run().expect("a reply");
    send(&mut app, msg);

    assert_eq!(app.rename, RenameState::Idle);
    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/b.md")));
    assert_eq!(mem.read(Path::new("/root/b.md")).unwrap(), b"a content");
    assert!(
        mem.read(Path::new("/root/a.md")).is_err(),
        "the old name must be gone"
    );
    assert_eq!(app.active_doc().file_name(), "b.md");
    assert_eq!(
        app.is_dirty(),
        dirty_before,
        "a rename must not change dirty state"
    );
}

/// A `rename_excl` I/O failure posts an error message, leaves `file_path`
/// alone, and returns the machine to `Idle`.
#[test]
fn a_rename_io_failure_posts_an_error_message_and_changes_nothing() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    let mut effects = rename_to(&mut app, "b");
    mem.fail_next(
        rune_vfs::OpKind::RenameExcl,
        std::io::ErrorKind::PermissionDenied,
    );
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");
    send(&mut app, cmd.run().expect("a reply"));

    assert_eq!(app.rename, RenameState::Idle);
    assert!(rune_tui::messages::newest_text(&app).is_some());
    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/a.md")));
    assert_eq!(mem.read(Path::new("/root/a.md")).unwrap(), b"a content");
}

/// Gotcha 1, re-checked under WP3: a failed rename returns the user to the
/// title holding the FULL typed name, extension included — never a name
/// corrupted by re-stripping the stem — and the undo history built while
/// typing survives (the field is never reseeded on a failure path).
#[test]
fn a_failed_rename_returns_to_the_title_with_the_typed_name_and_its_undo_history() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    let mut effects = rename_to(&mut app, "newname");
    mem.fail_next(
        rune_vfs::OpKind::RenameExcl,
        std::io::ErrorKind::PermissionDenied,
    );
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");
    send(&mut app, cmd.run().expect("a reply"));

    assert_eq!(app.rename, RenameState::Idle);
    assert_eq!(
        app.focus(),
        Pane::Title,
        "a failed rename returns focus to the title"
    );
    assert_eq!(
        app.title.text(),
        "newname.md",
        "the typed name survives verbatim, extension included"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some(),
        "the failure posts an error message"
    );

    // The message is non-modal (plan WP1) — it never captures the
    // keyboard, so the title (still holding the typed name underneath)
    // sees every keystroke exactly as if no message had been posted.
    assert_eq!(app.focus(), Pane::Title);

    send(&mut app, ctrl('z'));
    assert_ne!(
        app.title.text(),
        "newname.md",
        "the undo history built while typing must still work"
    );
}

/// A second commit while one is in flight is REFUSED, never queued.
#[test]
fn a_second_commit_while_one_is_in_flight_is_refused() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    let first = rename_to(&mut app, "b");
    assert_eq!(
        first
            .cmds
            .iter()
            .filter(|c| c.kind() == CmdKind::Rename)
            .count(),
        1
    );
    assert!(matches!(app.rename, RenameState::Committing { .. }));

    let second = rename_to(&mut app, "c");
    assert_eq!(
        second
            .cmds
            .iter()
            .filter(|c| c.kind() == CmdKind::Rename)
            .count(),
        0,
        "the second commit must enqueue nothing"
    );
}

// ── Draft naming ────────────────────────────────────────────────────────

/// Decision 9: a pathless draft has no name to derive a stem from, so it
/// seeds with a bare `.md` — an empty stem plus the extension every rune
/// document gets — and the gate starts UNLOCKED, since an empty stem has
/// nothing to fence off.
#[test]
fn a_draft_seeds_with_a_dotted_md_and_an_unlocked_gate() {
    let mem = Arc::new(Mem::new());
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new("draft body"), None, vfs, None);

    send(&mut app, ctrl('r'));

    assert_eq!(app.title.text(), ".md");
    assert!(
        app.title.ext_unlocked(),
        "an empty stem has nothing to fence off"
    );
}

/// Enter on a pathless draft is a CREATE, not a rename: no `Rename` state
/// survives, and the file is published no-clobber.
#[test]
fn naming_a_draft_creates_the_file() {
    let mem = Arc::new(Mem::new());
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new("draft body"), None, vfs, None);

    send(&mut app, ctrl('r'));
    type_text(&mut app, "fresh");
    let mut effects = send(&mut app, plain(KeyCode::Enter));

    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a create Cmd");
    send(&mut app, cmd.run().expect("a reply"));

    assert_eq!(app.rename, RenameState::Idle);
    let path = active_path(&app).expect("the draft must now be bound");
    assert_eq!(path.file_name().unwrap(), "fresh.md");
    assert_eq!(mem.read(&path).unwrap(), b"draft body");
    assert_eq!(
        app.active_doc().file_name(),
        "fresh.md",
        "the no-store create ack must switch the title to the real filename"
    );
}

/// A draft name that collides gives a FOOTER refusal and never a
/// `RenameCollision` guard — offering `[R]eplace` would overwrite a foreign
/// file with a buffer that has no CAS baseline (§1.4.7).
#[test]
fn a_colliding_draft_name_refuses_in_the_footer_with_no_guard() {
    let mem = Arc::new(Mem::new());
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new("draft body"), None, vfs, None);
    // An absolute spelling: `Mem::resolve` now lexically normalizes
    // (WP1.S6), so a bare relative/dotted spelling here would be published
    // under a different key than this test's own closing `mem.read`
    // (which never resolves) looks up.
    let existing = Path::new("/taken.md");
    mem.save_atomic(existing, b"someone else's file")
        .expect("seed");

    send(&mut app, ctrl('r'));
    type_text(&mut app, "taken");
    let mut effects = send(&mut app, plain(KeyCode::Enter));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a create Cmd");
    send(&mut app, cmd.run().expect("a reply"));

    assert_eq!(app.rename, RenameState::Idle);
    assert!(
        app.guard.is_none(),
        "a draft collision must never raise a guard"
    );
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|m| m.contains("already exists")),
        "got {:?}",
        app.status_message
    );
    assert!(
        active_path(&app).is_none(),
        "a refused create must leave the draft untitled (a later save must \
         not overwrite the winner)"
    );
    assert_eq!(mem.read(existing).unwrap(), b"someone else's file");
}

/// Regression: naming a store-bound draft (^R -> Enter, routed through
/// `save::bind_new_now`'s materialize) must switch the title to the real
/// filename via the SAME `Document::bind_path` chokepoint the no-store
/// route (`naming_a_draft_creates_the_file`, above) already goes through —
/// not just set `file_path` while leaving a stale `display_name` override
/// (e.g. "Untitled 1") to shadow it forever.
#[test]
fn store_bound_draft_create_ack_clears_the_untitled_display_name() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/seed.md"), b"seed")
        .expect("seed");
    let (mut app, rx) = rename_common::draft_app_with_store(&mem);

    send(&mut app, ctrl('r'));
    type_text(&mut app, "fresh");
    send(&mut app, plain(KeyCode::Enter));

    // WP7: the store-backed create is now a three-hop round trip —
    // `MaterializePrepare`'s ack spawns the caller-side `vfs` `Cmd`
    // (`handle_prepare_ack`), which itself replies with a `Msg` that
    // enqueues `MaterializeRecord`.
    let prep_evt = rename_common::next_event(&rx);
    let mut effects = send(&mut app, rune_tui::runtime::Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = rename_common::next_event(&rx);
    send(&mut app, rune_tui::runtime::Msg::Db(record_evt));

    assert_eq!(app.rename, RenameState::Idle);
    let path = active_path(&app).expect("the draft must now be bound");
    assert_eq!(path.file_name().unwrap(), "fresh.md");
    assert_eq!(
        app.active_doc().file_name(),
        "fresh.md",
        "a store-bound create ack must clear the untitled display_name override"
    );
}
