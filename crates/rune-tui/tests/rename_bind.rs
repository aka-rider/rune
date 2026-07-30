//! Rename "Done when" tests: focus/typing, the refusals, the end-to-end
//! no-store rename, and draft naming — TODO.md's §1.6 split of the
//! original `rename.rs`. The collision guard/hazard-1 tests, the
//! store-backed `[R]eplace` path, and the WP2 focus-loss-is-the-commit-
//! chokepoint suite live in the siblings `rename_collision.rs`/
//! `rename_replace.rs`/`rename_focus.rs`; all four pull shared fixtures
//! from `rename_common`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_tui::app::App;
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::runtime::CmdKind;
use rune_tui::title::ext_split;

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, Vfs};

use rename_common::{
    active_path, app_with, assert_refused, ctrl, key, plain, rename_to, seeded_vfs, send,
    type_text,
};

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

// ── The extension gate ──────────────────────────────────────────────────

/// The Right-at-end-of-stem gesture unlocks the extension without moving
/// the cursor — a further motion can then reach into it.
#[test]
fn right_at_the_end_of_the_stem_unlocks_the_extension_without_moving_the_cursor() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    assert!(!app.title.ext_unlocked(), "seeded with a stem: starts locked");
    let cursor_before = app.title.field().cursor().position;

    send(&mut app, plain(KeyCode::Right));

    assert!(app.title.ext_unlocked(), "Right at the split unlocks");
    assert_eq!(
        app.title.field().cursor().position,
        cursor_before,
        "the gate unlocks without moving the cursor"
    );

    send(&mut app, plain(KeyCode::End));
    assert_eq!(
        app.title.field().cursor().position,
        app.title.text().len(),
        "End can now reach past the split, into the unlocked extension"
    );
}

/// Locked, `End` stops at the split (never inside the extension), and
/// `Delete` right there is a no-op — the extension is fenced off, not just
/// dimmed.
#[test]
fn the_extension_is_fenced_off_until_unlocked() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    send(&mut app, plain(KeyCode::End));
    assert_eq!(
        app.title.field().cursor().position,
        ext_split(app.title.text()),
        "End stops at the split while locked"
    );

    send(&mut app, plain(KeyCode::Delete));
    assert_eq!(
        app.title.text(),
        "a.md",
        "DeleteRight at the split is a no-op while locked"
    );
}

// ── Editing: word motion, selection, undo ───────────────────────────────

/// ⌥→/⌥⇧→ resolve through the same `EDITOR_BINDINGS` table the document
/// editor uses, windowed to the (locked) stem.
#[test]
fn word_motion_and_shift_selection_work_in_the_title() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    send(&mut app, ctrl('a'));
    send(&mut app, plain(KeyCode::Backspace));
    type_text(&mut app, "two words");
    assert_eq!(app.title.text(), "two words.md");

    send(&mut app, plain(KeyCode::Home));
    send(
        &mut app,
        key(
            KeyCode::Right,
            Mods {
                alt: true,
                ..Mods::NONE
            },
        ),
    );
    assert_eq!(
        app.title.field().cursor().position,
        3,
        "word-right stops at the end of 'two'"
    );

    send(
        &mut app,
        key(
            KeyCode::Right,
            Mods {
                alt: true,
                shift: true,
                ..Mods::NONE
            },
        ),
    );
    assert_eq!(
        app.title.field().cursor().position,
        9,
        "shift-word-right extends to the end of 'words', clamped by the locked window"
    );
    assert_eq!(app.title.field().selected_text(), " words");
}

/// The title's own `⌘Z` undoes typing WITHOUT ever touching the active
/// document's journal (§12: "the title field is unjournaled").
#[test]
fn undo_in_the_title_never_touches_the_document_journal() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let doc_journal_pos_before = app.active_doc().journal.pos();

    send(&mut app, ctrl('r'));
    type_text(&mut app, "xyz");
    assert_eq!(app.title.text(), "axyz.md");

    send(&mut app, ctrl('z'));

    assert_eq!(app.title.text(), "axy.md");
    assert_eq!(
        app.active_doc().journal.pos(),
        doc_journal_pos_before,
        "the title's own undo must never touch the document journal"
    );
}

// ── Refusals ────────────────────────────────────────────────────────────

/// Decision 12: a read-only document's title cannot be focused AT ALL — the
/// refusal now happens at `^r` itself (`App::focus_title`), before there is
/// ever anything to type. Focusing the Help document's title would
/// otherwise hold the user in a field describing a document they can never
/// rename; removing the illegal state beats guarding it later inside
/// `rename::begin`.
#[test]
fn a_read_only_document_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    app.active_doc_mut().read_only = true;
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));

    assert_eq!(app.focus(), Pane::Editor, "the title must never gain focus");
    assert_eq!(
        app.status_message.as_deref(),
        Some("this document is read-only")
    );
    assert_eq!(app.active_doc().buffer.content(), before);
}

/// Decision 12: the Help document is read-only, so its title can never gain
/// focus at all — `^r` refuses with a status instead, and the title row
/// still reads "Help".
#[test]
fn the_help_document_refuses_title_focus() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, plain(KeyCode::F1));
    assert_eq!(app.active_doc().file_name(), "Help");

    send(&mut app, ctrl('r'));

    assert_eq!(
        app.focus(),
        Pane::Editor,
        "a read-only document's title must never gain focus"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("this document is read-only")
    );
    assert_eq!(
        app.active_doc().file_name(),
        "Help",
        "the title row must still read Help"
    );
}

/// The no-store `save_cmd` captures `path` in its closure and would
/// republish at the OLD name, so a rename mid-save is refused.
#[test]
fn a_save_in_flight_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    app.active_doc_mut().save_in_flight = true;
    let before = app.active_doc().buffer.content().to_string();

    let effects = rename_to(&mut app, "b");
    assert_refused(&app, &effects, &before);
}

/// A fully empty name is now only reachable with the extension gate
/// unlocked — locked, the extension always leaves at least a dot behind
/// (`title::TitleField::window`'s fenced-off tail), and `.md` alone is a
/// perfectly valid dotfile name, not an empty one.
#[test]
fn an_empty_name_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));
    send(&mut app, plain(KeyCode::Right)); // unlock: cursor sits at the split
    send(&mut app, ctrl('a'));
    send(&mut app, plain(KeyCode::Backspace));
    assert_eq!(app.title.text(), "");
    let effects = send(&mut app, plain(KeyCode::Enter));
    assert_refused(&app, &effects, &before);
}

/// `/` is filtered at the keystroke, so it can never even reach the name —
/// the field's own validation is the second line of defence.
#[test]
fn a_slash_never_enters_the_field() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    type_text(&mut app, "b/c");
    assert_eq!(
        app.title.text(),
        "abc.md",
        "'/' must be filtered at the keystroke"
    );
}

/// Committing an unchanged name is a plain refocus, never a rename of a
/// file onto its own path.
#[test]
fn an_unchanged_name_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));
    let effects = send(&mut app, plain(KeyCode::Enter));

    assert_eq!(app.focus(), Pane::Editor);
    assert_refused(&app, &effects, &before);
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
    app.active_doc_mut().mark_dirty_from_hydration();
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

/// A `rename_excl` I/O failure surfaces as an error modal, leaves
/// `file_path` alone, and returns the machine to `Idle`.
#[test]
fn a_rename_io_failure_raises_the_error_modal_and_changes_nothing() {
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
    assert!(matches!(app.modal, Some(rune_tui::banner::Modal::Error(_))));
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
    assert!(app.modal.is_some(), "the failure raises an error modal");

    // Stage 1 gives the modal the keyboard first — dismiss it before the
    // title (still holding the typed name underneath) can see a keystroke.
    send(&mut app, plain(KeyCode::Escape));
    assert!(app.modal.is_none());
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

/// `lessrc.md` -> `lessrc`: the whole point of editing the extension
/// in-line. `.` is word-forming (gotcha 18), so this name is deliberately
/// NOT `a.md` — a single ⌥← from the end would otherwise jump straight
/// past the dot instead of exercising Backspace across it.
#[test]
fn deleting_the_extension_renames_to_an_extensionless_file() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/lessrc.md"), b"content")
        .expect("seed lessrc.md");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new("content"),
        Some(PathBuf::from("/root/lessrc.md")),
        vfs,
        None,
    );

    send(&mut app, ctrl('r'));
    assert_eq!(app.title.text(), "lessrc.md");
    send(&mut app, plain(KeyCode::Right)); // unlock the extension
    send(&mut app, plain(KeyCode::End));
    send(&mut app, plain(KeyCode::Backspace));
    send(&mut app, plain(KeyCode::Backspace));
    send(&mut app, plain(KeyCode::Backspace));
    assert_eq!(app.title.text(), "lessrc");

    let mut effects = send(&mut app, plain(KeyCode::Enter));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");
    send(&mut app, cmd.run().expect("a reply"));

    assert_eq!(
        active_path(&app).as_deref(),
        Some(Path::new("/root/lessrc"))
    );
    assert_eq!(mem.read(Path::new("/root/lessrc")).unwrap(), b"content");
    assert!(
        mem.read(Path::new("/root/lessrc.md")).is_err(),
        "the old name must be gone"
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
        app.modal.is_none(),
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
