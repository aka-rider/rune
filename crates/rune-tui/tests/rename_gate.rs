//! The title's extension gate, and the field's own word-motion/selection/
//! undo editing — split out of `rename_bind.rs` (500-line
//! budget), driven through `rune_fuzz::Session`. Both sections were added
//! by the same extension-gate package that grew that file past the ceiling.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;
use std::sync::Arc;

use rune_core::coords::BufferOffset;
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::rename::RenameState;
use rune_tui::title::ext_split;
use rune_vfs::{Mem, Vfs, VfsTestExt};

use rename_common::{
    active_path, bound_session, ctrl_key, key_input, open_title, plain_key, store_session,
};

// ── The extension gate ──────────────────────────────────────────────────

/// The Right-at-end-of-stem gesture unlocks the extension without moving
/// the cursor — a further motion can then reach into it.
#[test]
fn right_at_the_end_of_the_stem_unlocks_the_extension_without_moving_the_cursor() {
    let (mut session, _mem) = bound_session();

    open_title(&mut session);
    assert!(
        !session.app().title.ext_unlocked(),
        "seeded with a stem: starts locked"
    );
    let cursor_before = session.app().title.field().cursor().position;

    assert!(session.key(plain_key(KeyCode::Right)).is_none());

    assert!(
        session.app().title.ext_unlocked(),
        "Right at the split unlocks"
    );
    assert_eq!(
        session.app().title.field().cursor().position,
        cursor_before,
        "the gate unlocks without moving the cursor"
    );

    assert!(session.key(plain_key(KeyCode::End)).is_none());
    assert_eq!(
        session.app().title.field().cursor().position,
        BufferOffset(session.app().title.text().len()),
        "End can now reach past the split, into the unlocked extension"
    );
}

/// Locked, `End` stops at the split (never inside the extension), and
/// `Delete` right there is a no-op — the extension is fenced off, not just
/// dimmed.
#[test]
fn the_extension_is_fenced_off_until_unlocked() {
    let (mut session, _mem) = bound_session();

    open_title(&mut session);
    assert!(session.key(plain_key(KeyCode::End)).is_none());
    assert_eq!(
        session.app().title.field().cursor().position,
        BufferOffset(ext_split(session.app().title.text())),
        "End stops at the split while locked"
    );

    assert!(session.key(plain_key(KeyCode::Delete)).is_none());
    assert_eq!(
        session.app().title.text(),
        "a.md",
        "DeleteRight at the split is a no-op while locked"
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
    let mut session = store_session(&mem, "/root/lessrc.md");

    open_title(&mut session);
    assert_eq!(session.app().title.text(), "lessrc.md");
    assert!(session.key(plain_key(KeyCode::Right)).is_none()); // unlock the extension
    assert!(session.key(plain_key(KeyCode::End)).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert_eq!(session.app().title.text(), "lessrc");

    assert!(session.key(plain_key(KeyCode::Enter)).is_none());
    assert!(matches!(
        session.app().rename,
        RenameState::Committing { .. }
    ));
    assert!(session.deliver_db_all().is_none());

    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(
        active_path(session.app()).as_deref(),
        Some(Path::new("/root/lessrc"))
    );
    assert_eq!(mem.read(Path::new("/root/lessrc")).unwrap(), b"content");
    assert!(
        mem.read(Path::new("/root/lessrc.md")).is_err(),
        "the old name must be gone"
    );
}

// ── Editing: word motion, selection, undo ───────────────────────────────

/// ⌥→/⌥⇧→ resolve through the same `EDITOR_BINDINGS` table the document
/// editor uses, windowed to the (locked) stem.
#[test]
fn word_motion_and_shift_selection_work_in_the_title() {
    let (mut session, _mem) = bound_session();

    open_title(&mut session);
    assert!(session.key(ctrl_key('a')).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert!(session.type_("two words").is_none());
    assert_eq!(session.app().title.text(), "two words.md");

    assert!(session.key(plain_key(KeyCode::Home)).is_none());
    assert!(
        session
            .key(key_input(
                KeyCode::Right,
                Mods {
                    alt: true,
                    ..Mods::NONE
                },
            ))
            .is_none()
    );
    assert_eq!(
        session.app().title.field().cursor().position,
        BufferOffset(3),
        "word-right stops at the end of 'two'"
    );

    assert!(
        session
            .key(key_input(
                KeyCode::Right,
                Mods {
                    alt: true,
                    shift: true,
                    ..Mods::NONE
                },
            ))
            .is_none()
    );
    assert_eq!(
        session.app().title.field().cursor().position,
        BufferOffset(9),
        "shift-word-right extends to the end of 'words', clamped by the locked window"
    );
    assert_eq!(session.app().title.field().selected_text(), " words");
}

/// The title's own `⌘Z` undoes typing WITHOUT ever touching the active
/// document's journal — the title field is unjournaled.
#[test]
fn undo_in_the_title_never_touches_the_document_journal() {
    let (mut session, _mem) = bound_session();
    let doc_journal_pos_before = session.app().active_doc().journal.pos();

    open_title(&mut session);
    assert!(session.type_("xyz").is_none());
    assert_eq!(session.app().title.text(), "axyz.md");

    assert!(session.key(ctrl_key('z')).is_none());

    assert_eq!(session.app().title.text(), "axy.md");
    assert_eq!(
        session.app().active_doc().journal.pos(),
        doc_journal_pos_before,
        "the title's own undo must never touch the document journal"
    );
}

/// Regression: typing an extension onto a name that has none must produce
/// the name the user typed.
///
/// The gate's window is derived from the live text on every keystroke, so
/// the moment the user types the `.` themselves the split jumps to it and
/// the window shrinks to exclude the cursor's own position — stranding the
/// caret outside the editable range and folding every following character
/// back in front of the dot. `README` + `.md` became `READMEmd.`, an
/// `is_valid_name`-passing name that commits to disk on blur.
#[test]
fn typing_an_extension_onto_an_extensionless_name_keeps_the_characters_in_order() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/README"), b"readme")
        .expect("seed README");
    let mut session = store_session(&mem, "/root/README");

    open_title(&mut session);
    assert_eq!(session.app().title.text(), "README");

    assert!(session.type_(".md").is_none());

    assert_eq!(
        session.app().title.text(),
        "README.md",
        "each typed character must land where the caret actually is"
    );
}
