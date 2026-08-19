//! Explorer behavior on symlinked rows: a preview binds the document under
//! the resolved path, so hovering a link and then opening it — or opening
//! the same file once by each of its two spellings — can never leave two
//! documents with two independent dirty states behind.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod explorer_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_fuzz::Session;
use rune_tui::app;
use rune_tui::explorer_keys;
use rune_tui::keymap::{KeyCode, KeyOutcome};
use rune_tui::runtime::{CmdKind, Effects};
use rune_vfs::Mem;

use explorer_common::{drive_load_explorer, key, open_seeded, seeded_vfs};

/// The shared `/root` fixture plus `link.md` (a link to `b.md`) and
/// `broken.md` (a link to a file that was never created).
fn linked_vfs() -> Arc<Mem> {
    let mem = seeded_vfs();
    mem.symlink(Path::new("/root/link.md"), Path::new("/root/b.md"))
        .expect("seed a symlink to b.md");
    mem.symlink(Path::new("/root/broken.md"), Path::new("/root/gone.md"))
        .expect("seed a broken symlink");
    mem
}

fn row_index(session: &Session, name: &str) -> usize {
    session
        .app()
        .explorer
        .entries
        .iter()
        .position(|e| e.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
}

/// Arrows the cursor DOWN onto `name` through the real key handler, so the
/// live-preview hook fires exactly the way a user's arrow key fires it.
fn arrow_onto(session: &mut Session, name: &str) -> Effects {
    session.app_mut().explorer.nav.cursor = row_index(session, name) - 1;
    let mut effects = Effects::default();
    assert_eq!(
        explorer_keys::handle_key(session.app_mut(), key(KeyCode::Down), &mut effects),
        KeyOutcome::Consumed
    );
    effects
}

fn run_cmds(session: &mut Session, effects: &mut Effects) {
    for cmd in std::mem::take(&mut effects.cmds) {
        if let Some(msg) = cmd.run() {
            app::update(session.app_mut(), msg, effects);
        }
    }
}

fn press_enter(session: &mut Session) -> Effects {
    let mut effects = Effects::default();
    assert_eq!(
        explorer_keys::handle_key(session.app_mut(), key(KeyCode::Enter), &mut effects),
        KeyOutcome::Consumed
    );
    effects
}

fn documents_at(session: &Session, path: &str) -> usize {
    let app = session.app();
    app.documents
        .order()
        .iter()
        .filter(|id| {
            app.doc(**id).and_then(|doc| doc.file_path.as_deref()) == Some(Path::new(path))
        })
        .count()
}

#[test]
fn previewing_a_symlink_and_then_opening_it_yields_exactly_one_document() {
    let mem = linked_vfs();
    let mut session = open_seeded(&mem);
    drive_load_explorer(&mut session);

    let mut effects = arrow_onto(&mut session, "link.md");
    run_cmds(&mut session, &mut effects);
    let mut effects = press_enter(&mut session);
    run_cmds(&mut session, &mut effects);

    assert_eq!(
        documents_at(&session, "/root/b.md"),
        1,
        "the previewed link and the opened link are one document"
    );
    assert_eq!(
        documents_at(&session, "/root/link.md"),
        0,
        "no document ever binds the unresolved link path"
    );
    assert_eq!(
        session.app().active_doc().file_path.as_deref(),
        Some(Path::new("/root/b.md"))
    );
    assert_eq!(session.app().active_doc().buffer.content(), "b content");
}

#[test]
fn opening_a_file_by_its_real_path_and_by_a_symlink_reactivates_the_same_document() {
    let mem = linked_vfs();
    let mut session = open_seeded(&mem);
    drive_load_explorer(&mut session);

    let mut effects = arrow_onto(&mut session, "b.md");
    run_cmds(&mut session, &mut effects);
    let mut effects = press_enter(&mut session);
    run_cmds(&mut session, &mut effects);
    let opened = session.app().active;
    let documents = session.app().documents.len();

    let mut effects = arrow_onto(&mut session, "link.md");
    run_cmds(&mut session, &mut effects);
    let mut effects = press_enter(&mut session);
    run_cmds(&mut session, &mut effects);

    assert_eq!(
        session.app().active,
        opened,
        "the link reactivates the document its target already has"
    );
    assert_eq!(
        session.app().documents.len(),
        documents,
        "no second tab for the second spelling"
    );
}

#[test]
fn a_broken_symlink_never_starts_a_preview() {
    let mem = linked_vfs();
    let mut session = open_seeded(&mem);
    drive_load_explorer(&mut session);

    let effects = arrow_onto(&mut session, "broken.md");

    assert!(
        !effects.cmds.iter().any(|c| c.kind() == CmdKind::ReadFile),
        "there is nothing behind a broken link to read"
    );
    assert_eq!(documents_at(&session, "/root/broken.md"), 0);
    assert_eq!(documents_at(&session, "/root/gone.md"), 0);
}

#[test]
fn a_symlinked_row_is_previewed_under_its_target_path() {
    let mem = linked_vfs();
    let mut session = open_seeded(&mem);
    drive_load_explorer(&mut session);

    let mut effects = arrow_onto(&mut session, "link.md");
    run_cmds(&mut session, &mut effects);

    let preview = session
        .app()
        .explorer
        .preview
        .expect("hovering a link mints a preview");
    assert_eq!(
        session
            .app()
            .doc(preview)
            .and_then(|doc| doc.file_path.clone()),
        Some(PathBuf::from("/root/b.md"))
    );
}
