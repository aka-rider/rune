//! The save/ack/dirty-flow tests that used to live in `src/save.rs` (moved
//! out in plan WP1.S5 to keep that file under the §1.6 line budget — every
//! item exercised here — `App`, `update`, `Msg`, `Effects`, `keymap`
//! types, `commands::edit::insert_char` — is already public, so this needs
//! no crate-internal access `#[cfg(test)]` had).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{App, update};
use rune_tui::commands::edit;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::{Disk, Mem, Vfs};

fn test_app() -> App {
    App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
}

#[test]
fn save_done_ok_advances_saved_version_and_clears_a_prior_save_failure() {
    let mut app = test_app();
    let id = app.active;
    let version = app.doc(id).unwrap().buffer.version();

    // A real prior save failure — the only kind of message the
    // provenance-aware clear below (review finding F2) is allowed to
    // dismiss.
    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Err("oops".to_string()),
        },
        &mut effects,
    );
    assert!(app.status_message.is_some());

    let mut effects2 = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Ok(()),
        },
        &mut effects2,
    );
    assert_eq!(app.doc(id).unwrap().saved_version, version);
    assert!(
        app.status_message.is_none(),
        "a successful save must clear the failure message ITS OWN save path set"
    );
}

/// Regression for F2: a successful save must not clear a status message
/// some OTHER subsystem set — e.g. an unresolved `Msg::Error` such as a
/// pbpaste failure the user hasn't dismissed yet.
#[test]
fn save_done_ok_does_not_clear_an_unrelated_status_message() {
    let mut app = test_app();
    let id = app.active;
    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::Error("pbpaste failed to run: No such file or directory".to_string()),
        &mut effects,
    );
    assert!(app.status_message.is_some());

    let version = app.doc(id).unwrap().buffer.version();
    let mut effects2 = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Ok(()),
        },
        &mut effects2,
    );

    assert_eq!(app.doc(id).unwrap().saved_version, version);
    assert!(
        app.status_message.is_some(),
        "a successful save must not clear an unrelated (non-save) status message"
    );
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|s| s.contains("pbpaste"))
    );
}

#[test]
fn save_done_err_surfaces_status_and_keeps_dirty() {
    let mut app = test_app();
    let id = app.active;
    app.doc_mut(id).unwrap().buffer = app.doc(id).unwrap().buffer.insert(0, "x");
    let before_saved = app.doc(id).unwrap().saved_version;
    let version = app.doc(id).unwrap().buffer.version();
    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Err("disk full".to_string()),
        },
        &mut effects,
    );
    assert_eq!(app.doc(id).unwrap().saved_version, before_saved);
    assert!(app.is_dirty());
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|s| s.contains("disk full"))
    );
}

fn save_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('s'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    }
}

fn press_save(app: &mut App) -> Effects {
    let mut effects = Effects::default();
    update(app, Msg::Key(save_key()), &mut effects);
    effects
}

fn settle_cmds(app: &mut App, effects: Effects) {
    for cmd in effects.cmds {
        if let Some(msg) = cmd.run() {
            let mut next = Effects::default();
            update(app, msg, &mut next);
            settle_cmds(app, next);
        }
    }
}

#[test]
fn save_persists_exact_bytes_for_crlf_bom_and_no_trailing_newline_fixtures() {
    for content in ["a\r\nb\r\n", "\u{feff}hello", "no trailing newline"] {
        let vfs = Arc::new(Mem::new());
        let path = PathBuf::from("/doc.md");
        let mut app = App::new(
            Buffer::new(content),
            Some(path.clone()),
            Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
            None,
        );
        let id = app.active;
        app.doc_mut(id).unwrap().saved_version = 0;

        let effects = press_save(&mut app);
        assert_eq!(effects.cmds.len(), 1, "one save Cmd must be spawned");
        settle_cmds(&mut app, effects);

        let saved = vfs.read(&path).expect("save must have written the file");
        assert_eq!(
            saved,
            content.as_bytes(),
            "saved bytes must be byte-identical to the buffer, verbatim"
        );
        assert!(!app.is_dirty());
    }
}

#[test]
fn save_failure_surfaces_a_status_error_and_keeps_dirty() {
    let vfs = Arc::new(Mem::new());
    vfs.fail_next_save(std::io::ErrorKind::Other);
    let path = PathBuf::from("/doc.md");
    let mut app = App::new(
        Buffer::new("hello"),
        Some(path),
        Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
        None,
    );
    let id = app.active;
    app.doc_mut(id).unwrap().saved_version = 0;

    let effects = press_save(&mut app);
    settle_cmds(&mut app, effects);

    assert!(app.is_dirty());
    assert!(
        app.status_message.is_some(),
        "a failed save must surface a status-line error"
    );
}

#[test]
fn a_second_save_press_while_one_is_in_flight_is_a_no_op() {
    let mut app = App::new(
        Buffer::new("hello"),
        Some(PathBuf::from("/doc.md")),
        Arc::new(Mem::new()),
        None,
    );
    let id = app.active;
    app.doc_mut(id).unwrap().buffer = app.doc(id).unwrap().buffer.insert(0, "x"); // makes it dirty

    let effects = press_save(&mut app);
    assert_eq!(effects.cmds.len(), 1);
    assert!(app.doc(id).unwrap().save_in_flight);

    let effects2 = press_save(&mut app);
    assert!(
        effects2.cmds.is_empty(),
        "a save already in flight must not spawn a second save Cmd"
    );
    assert!(app.doc(id).unwrap().save_in_flight);
}

#[test]
fn an_edit_during_a_save_keeps_the_buffer_dirty_once_the_save_completes() {
    let vfs = Arc::new(Mem::new());
    let path = PathBuf::from("/doc.md");
    let mut app = App::new(
        Buffer::new("hello"),
        Some(path),
        Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
        None,
    );
    let id = app.active;
    app.doc_mut(id).unwrap().saved_version = 0;

    let effects = press_save(&mut app); // captures the pre-edit version
    assert_eq!(effects.cmds.len(), 1);

    edit::insert_char(&mut app, id, '!');
    let after_edit_version = app.doc(id).unwrap().buffer.version();

    settle_cmds(&mut app, effects); // delivers SaveDone for the OLD version

    assert!(
        app.doc(id).unwrap().saved_version < after_edit_version,
        "SaveDone must only advance saved_version to the version IT saved, \
         not the buffer's current (post-edit) version"
    );
    assert!(
        app.is_dirty(),
        "an edit made during the in-flight save must leave the buffer dirty \
         once that save completes"
    );
}

#[test]
fn saving_a_path_that_does_not_exist_on_disk_creates_it_via_the_excl_path() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("rune-wp9-excl-{}-{n}.md", std::process::id()));
    let _ = std::fs::remove_file(&path); // in case a prior run left it behind
    assert!(!path.exists(), "the fixture path must not exist yet");

    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Disk);
    let mut app = App::new(
        Buffer::new("brand new file\n"),
        Some(path.clone()),
        vfs,
        None,
    );
    let id = app.active;
    app.doc_mut(id).unwrap().saved_version = 0;

    let effects = press_save(&mut app);
    settle_cmds(&mut app, effects);

    assert!(!app.is_dirty());
    let saved = std::fs::read(&path).expect("save must have created the file on disk");
    assert_eq!(saved, b"brand new file\n");

    let _ = std::fs::remove_file(&path);
}
