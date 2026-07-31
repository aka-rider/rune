//! The title's extension gate, and the field's own word-motion/selection/
//! undo editing — split out of `rename_bind.rs` (plan WP5, §1.6). Both
//! sections were added by the same extension-gate package that grew that
//! file past the ceiling.

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
use rune_tui::runtime::CmdKind;
use rune_tui::title::ext_split;

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, Vfs};

use rename_common::{active_path, app_with, ctrl, key, plain, seeded_vfs, send, type_text};

// ── The extension gate ──────────────────────────────────────────────────

/// The Right-at-end-of-stem gesture unlocks the extension without moving
/// the cursor — a further motion can then reach into it.
#[test]
fn right_at_the_end_of_the_stem_unlocks_the_extension_without_moving_the_cursor() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    assert!(
        !app.title.ext_unlocked(),
        "seeded with a stem: starts locked"
    );
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
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/README"), b"readme")
        .expect("seed README");
    let mut app = App::new(
        Buffer::new("readme"),
        Some(PathBuf::from("/root/README")),
        Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>,
        None,
    );

    send(&mut app, ctrl('r'));
    assert_eq!(app.title.text(), "README");

    type_text(&mut app, ".md");

    assert_eq!(
        app.title.text(),
        "README.md",
        "each typed character must land where the caret actually is"
    );
}
