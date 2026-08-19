//! Rename "Done when" tests: focus/typing, the end-to-end `Cmd`-route
//! rename, and draft naming — TODO.md's 500-line budget split of the
//! original `rename.rs`, driven through `rune_fuzz::Session`: the refusal
//! paths live in `rename_refusals.rs`, the extension gate and the field's
//! own editing in `rename_gate.rs`, copy/cut/paste in
//! `rename_clipboard.rs`, the collision guard in `rename_collision.rs`,
//! the store-backed `[R]eplace` path in `rename_replace.rs`, and the
//! focus-loss-is-the-commit-chokepoint suite in `rename_focus.rs`; all
//! seven pull shared fixtures from `rename_common`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod dirty_common;
mod rename_common;

use std::path::Path;

use rune_tui::keymap::KeyCode;
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_vfs::{Vfs, VfsTestExt};

use rename_common::{
    DOC_PATH, bound_session, commit_name, ctrl, ctrl_key, draft_session, name_draft, open_title,
    plain, plain_key, send, set_name, type_text, unbound_session,
};

// ── Focus and typing ────────────────────────────────────────────────────

/// `^r` focuses the title, seeded with the file's FULL NAME (extension
/// included), and typing there never touches the buffer — the
/// `PANE-NO-BLEED` property, asserted directly.
#[test]
fn ctrl_r_focuses_the_title_and_typing_never_touches_the_buffer() {
    let (mut session, _mem) = bound_session();
    let before = session.app().active_doc().buffer.content().to_string();

    open_title(&mut session);
    assert_eq!(
        session.app().title.text(),
        "a.md",
        "seeded with the full name, extension included"
    );

    assert!(session.type_("xyz").is_none());
    assert_eq!(session.app().title.text(), "axyz.md");
    assert_eq!(
        session.app().active_doc().buffer.content(),
        before,
        "a keystroke aimed at the file name must never reach the buffer"
    );
}

/// `Esc` reverts to the committed name and refocuses the editor without
/// renaming anything.
#[test]
fn escape_reverts_the_field_and_renames_nothing() {
    let (mut session, mem) = bound_session();

    open_title(&mut session);
    assert!(session.type_("zzz").is_none());
    assert!(session.key(plain_key(KeyCode::Escape)).is_none());

    assert_eq!(session.app().title.text(), "a.md");
    assert_eq!(session.app().focus(), Pane::Editor);
    assert_eq!(
        rename_common::active_path(session.app()).as_deref(),
        Some(Path::new(DOC_PATH))
    );
    assert!(mem.read(Path::new(DOC_PATH)).is_ok());
}

/// Up at the top of the buffer focuses the title (a contextual gesture, no
/// new binding).
#[test]
fn up_at_the_top_of_the_editor_focuses_the_title() {
    let (mut session, _mem) = bound_session();
    assert!(session.key(plain_key(KeyCode::Up)).is_none());
    assert_eq!(session.app().focus(), Pane::Title);
}

// ── End to end, the Cmd route ───────────────────────────────────────────

/// An unbound document's rename is one `rename_excl` `Cmd`: once its reply
/// is delivered, the file moves, the tab and title show the new name, and
/// `is_dirty()` is UNCHANGED — a rename is not a save.
#[test]
fn end_to_end_cmd_route_rename() {
    let (mut session, mem) = unbound_session();
    // Make the document dirty, so "stays dirty" is actually observable.
    let active = session.app().active;
    dirty_common::force_dirty(session.app_mut(), active);
    let dirty_before = session.app().dirty_for_render();
    assert!(dirty_before, "test setup: the document must be dirty");

    commit_name(&mut session, "b");
    assert!(matches!(
        session.app().rename,
        RenameState::Committing { .. }
    ));

    assert!(session.deliver().is_none());

    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(
        rename_common::active_path(session.app()).as_deref(),
        Some(Path::new("/root/b.md"))
    );
    assert_eq!(
        mem.read(Path::new("/root/b.md")).unwrap(),
        b"a content",
        "the moved file carries the published bytes — a rename never writes the buffer"
    );
    assert!(
        mem.read(Path::new(DOC_PATH)).is_err(),
        "the old name must be gone"
    );
    assert_eq!(session.app().active_doc().file_name(), "b.md");
    assert_eq!(
        session.app().dirty_for_render(),
        dirty_before,
        "a rename must not change dirty state"
    );
}

/// A rename whose publish took effect but whose durability confirmation
/// failed is still a success — the file moves and the tab/title update —
/// and the unconfirmed durability is WARNED about, never swallowed.
#[test]
fn a_durability_unconfirmed_rename_succeeds_and_warns() {
    let (mut session, mem) = unbound_session();

    commit_name(&mut session, "b");
    mem.fail_after(rune_vfs::OpKind::RenameExcl, std::io::ErrorKind::Other);
    assert!(session.deliver().is_none());

    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(
        rename_common::active_path(session.app()).as_deref(),
        Some(Path::new("/root/b.md")),
        "the rename must still land despite unconfirmed durability"
    );
    assert_eq!(mem.read(Path::new("/root/b.md")).unwrap(), b"a content");
    assert!(
        mem.read(Path::new(DOC_PATH)).is_err(),
        "the old name must be gone"
    );
    assert!(
        rune_tui::messages::newest_text(session.app())
            .is_some_and(|m| m.contains("durability unconfirmed")),
        "the unconfirmed durability must be warned about, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
}

/// A `rename_excl` I/O failure posts an error message, leaves `file_path`
/// alone, and returns the machine to `Idle`.
#[test]
fn a_rename_io_failure_posts_an_error_message_and_changes_nothing() {
    let (mut session, mem) = unbound_session();

    commit_name(&mut session, "b");
    mem.fail_next(
        rune_vfs::OpKind::RenameExcl,
        std::io::ErrorKind::PermissionDenied,
    );
    assert!(session.deliver().is_none());

    assert_eq!(session.app().rename, RenameState::Idle);
    assert!(rune_tui::messages::newest_text(session.app()).is_some());
    assert_eq!(
        rename_common::active_path(session.app()).as_deref(),
        Some(Path::new(DOC_PATH))
    );
    assert_eq!(mem.read(Path::new(DOC_PATH)).unwrap(), b"a content");
}

/// Gotcha 1: a failed rename returns the user to the
/// title holding the FULL typed name, extension included — never a name
/// corrupted by re-stripping the stem — and the undo history built while
/// typing survives (the field is never reseeded on a failure path).
#[test]
fn a_failed_rename_returns_to_the_title_with_the_typed_name_and_its_undo_history() {
    let (mut session, mem) = unbound_session();

    commit_name(&mut session, "newname");
    mem.fail_next(
        rune_vfs::OpKind::RenameExcl,
        std::io::ErrorKind::PermissionDenied,
    );
    assert!(session.deliver().is_none());

    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(
        session.app().focus(),
        Pane::Title,
        "a failed rename returns focus to the title"
    );
    assert_eq!(
        session.app().title.text(),
        "newname.md",
        "the typed name survives verbatim, extension included"
    );
    assert!(
        rune_tui::messages::newest_text(session.app()).is_some(),
        "the failure posts an error message"
    );

    // The message is non-modal — it never captures the keyboard, so the
    // title (still holding the typed name underneath) sees every keystroke
    // exactly as if no message had been posted.
    assert_eq!(session.app().focus(), Pane::Title);

    assert!(session.key(ctrl_key('z')).is_none());
    assert_ne!(
        session.app().title.text(),
        "newname.md",
        "the undo history built while typing must still work"
    );
}

/// A second commit while one is in flight is REFUSED, never queued.
#[test]
fn a_second_commit_while_one_is_in_flight_is_refused() {
    let (mut session, mem) = unbound_session();

    commit_name(&mut session, "b");
    assert!(matches!(
        session.app().rename,
        RenameState::Committing { .. }
    ));

    // The second commit is refused at the blur, so focus never releases
    // and the in-flight state is untouched.
    set_name(&mut session, "c");
    assert!(session.key(plain_key(KeyCode::Enter)).is_none());
    assert_eq!(session.app().focus(), Pane::Title);
    assert!(matches!(
        session.app().rename,
        RenameState::Committing { .. }
    ));
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("a rename is already in progress")
    );

    assert!(session.deliver().is_none());
    assert!(
        mem.read(Path::new("/root/b.md")).is_ok(),
        "the FIRST commit must land"
    );
    assert!(
        mem.read(Path::new("/root/c.md")).is_err(),
        "the second commit must have enqueued nothing"
    );
}

// ── Draft naming ────────────────────────────────────────────────────────

/// Decision 9: a pathless draft has no name to derive a stem from, so it
/// seeds with a bare `.md` — an empty stem plus the extension every rune
/// document gets — and the gate starts UNLOCKED, since an empty stem has
/// nothing to fence off.
#[test]
fn a_draft_seeds_with_a_dotted_md_and_an_unlocked_gate() {
    let (mut session, _mem) = draft_session();

    open_title(&mut session);

    assert_eq!(session.app().title.text(), ".md");
    assert!(
        session.app().title.ext_unlocked(),
        "an empty stem has nothing to fence off"
    );
}

/// Enter on an unbound pathless draft is a CREATE, not a rename: no
/// `Rename` state survives, and the file is published no-clobber.
#[test]
fn naming_a_draft_creates_the_file() {
    let (mut session, mem) = draft_session();
    assert!(session.type_("draft body").is_none());

    name_draft(&mut session, "fresh");
    assert!(session.deliver().is_none());

    assert_eq!(session.app().rename, RenameState::Idle);
    let path = rename_common::active_path(session.app()).expect("the draft must now be bound");
    assert_eq!(path.file_name().unwrap(), "fresh.md");
    assert_eq!(mem.read(&path).unwrap(), b"draft body");
    assert_eq!(
        session.app().active_doc().file_name(),
        "fresh.md",
        "the create ack must switch the title to the real filename"
    );
}

/// A draft create whose publish took effect but whose durability
/// confirmation failed is still a success — the file exists and the draft
/// binds — and the unconfirmed durability is WARNED about, never swallowed.
#[test]
fn a_durability_unconfirmed_draft_create_succeeds_and_warns() {
    let (mut session, mem) = draft_session();
    assert!(session.type_("draft body").is_none());

    name_draft(&mut session, "fresh");
    mem.fail_after(rune_vfs::OpKind::RenameExcl, std::io::ErrorKind::Other);
    assert!(session.deliver().is_none());

    assert_eq!(session.app().rename, RenameState::Idle);
    let path = rename_common::active_path(session.app())
        .expect("the draft must be bound despite unconfirmed durability");
    assert_eq!(path.file_name().unwrap(), "fresh.md");
    assert_eq!(mem.read(&path).unwrap(), b"draft body");
    assert!(
        rune_tui::messages::newest_text(session.app())
            .is_some_and(|m| m.contains("durability unconfirmed")),
        "the unconfirmed durability must be warned about, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
}

/// A draft name that collides gives a FOOTER refusal and never a
/// `RenameCollision` guard — offering `[R]eplace` would overwrite a foreign
/// file with a buffer that has no CAS baseline.
#[test]
fn a_colliding_draft_name_refuses_in_the_footer_with_no_guard() {
    let (mut session, mem) = draft_session();
    assert!(session.type_("draft body").is_none());
    // The create target joins `explorer::initial_root` — computed from the
    // live app rather than hard-coded, so the seed collides with the very
    // path the create will actually claim.
    let existing = rune_tui::explorer::initial_root(session.app()).join("taken.md");
    mem.save_atomic(&existing, b"someone else's file")
        .expect("seed");

    name_draft(&mut session, "taken");
    assert!(session.deliver().is_none());

    assert_eq!(session.app().rename, RenameState::Idle);
    assert!(
        session.app().guard.is_none(),
        "a draft collision must never raise a guard"
    );
    assert!(
        rune_tui::messages::newest_text(session.app())
            .is_some_and(|m| m.contains("already exists")),
        "got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
    assert!(
        rename_common::active_path(session.app()).is_none(),
        "a refused create must leave the draft untitled (a later save must \
         not overwrite the winner)"
    );
    assert_eq!(mem.read(&existing).unwrap(), b"someone else's file");
}

/// Regression: naming a store-bound draft (^R -> Enter, routed through
/// `save::bind_new_now`'s materialize) must switch the title to the real
/// filename via the SAME `Document::bind_path` chokepoint the unbound
/// route (`naming_a_draft_creates_the_file`, above) already goes through —
/// not just set `file_path` while leaving a stale `display_name` override
/// (e.g. "Untitled 1") to shadow it forever.
#[test]
fn store_bound_draft_create_ack_clears_the_untitled_display_name() {
    let mem = std::sync::Arc::new(rune_vfs::Mem::new());
    let (mut app, bridge) = rename_common::draft_app_with_store(&mem);

    send(&mut app, ctrl('r'));
    type_text(&mut app, "fresh");
    send(&mut app, plain(KeyCode::Enter));

    // The store-backed create is a three-hop round trip —
    // `MaterializePrepare`'s ack spawns the caller-side `vfs` `Cmd`
    // (`handle_prepare_ack`), which itself replies with a `Msg` that
    // enqueues `MaterializeRecord`.
    let prep_evt = rename_common::wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, rune_tui::runtime::Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == rune_tui::runtime::CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = rename_common::wait_for_materialize_record(&bridge);
    send(&mut app, rune_tui::runtime::Msg::Db(record_evt));

    assert_eq!(app.rename, RenameState::Idle);
    let path = rename_common::active_path(&app).expect("the draft must now be bound");
    assert_eq!(path.file_name().unwrap(), "fresh.md");
    assert_eq!(
        app.active_doc().file_name(),
        "fresh.md",
        "a store-bound create ack must clear the untitled display_name override"
    );
}

/// A9: naming a STORE-BOUND pathless draft into a collision must reach the
/// very same footer-only refusal an unbound draft gets
/// (`a_colliding_draft_name_refuses_in_the_footer_with_no_guard`, above) —
/// never the unanswerable `RenameCollision` Guard, since there is no CAS
/// baseline for a target this draft has never claimed — and must leave the
/// user able to retype immediately: focus back in the title, not stranded
/// in the Editor with the old placeholder name still showing.
#[test]
fn a_colliding_store_bound_draft_name_refuses_in_the_footer_and_returns_focus_to_the_title() {
    let mem = std::sync::Arc::new(rune_vfs::Mem::new());
    let (mut app, bridge) = rename_common::draft_app_with_store(&mem);
    // The create target joins `explorer::initial_root` — computed from the
    // live app rather than hard-coded, so the seed collides with the very
    // path the create will actually claim.
    let existing = rune_tui::explorer::initial_root(&app).join("taken.md");
    mem.save_atomic(&existing, b"someone else's file")
        .expect("seed");

    send(&mut app, ctrl('r'));
    type_text(&mut app, "taken");
    send(&mut app, plain(KeyCode::Enter));

    // The same three-hop round trip as the successful create above.
    let prep_evt = rename_common::wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, rune_tui::runtime::Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == rune_tui::runtime::CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = rename_common::wait_for_materialize_record(&bridge);
    send(&mut app, rune_tui::runtime::Msg::Db(record_evt));

    assert!(
        app.guard.is_none(),
        "a store-bound draft collision must never raise a guard"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|m| m.contains("already exists")),
        "got {:?}",
        rune_tui::messages::newest_text(&app)
    );
    assert_eq!(
        app.focus(),
        Pane::Title,
        "the user must be returned straight to the title to retype the name"
    );
    assert_eq!(mem.read(&existing).unwrap(), b"someone else's file");
}
