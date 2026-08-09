//! The save/ack/dirty-flow tests that used to live in `src/save.rs` (moved
//! out in plan WP1.S5 to keep that file under the 500-line budget — every
//! item exercised here — `App`, `update`, `Msg`, `Effects`, `keymap`
//! types, `commands::edit::insert_char` — is already public, so this needs
//! no crate-internal access `#[cfg(test)]` had).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod dirty_common;

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{App, update};
use rune_tui::commands::edit;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{CmdKind, Effects, Msg};

/// A refused save posts a message, and an open message pane arms its own
/// auto-collapse timer — so "no save was issued" is a claim about the save
/// Cmd specifically, never about the effect list being empty.
fn spawns_a_save(effects: &Effects) -> bool {
    effects.cmds.iter().any(|cmd| cmd.kind() == CmdKind::Save)
}
use rune_vfs::{Disk, Mem, Vfs};

fn test_app() -> App {
    App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
}

/// A publish whose durability confirmation failed is still a SUCCESS —
/// reported as saved, with a warning, never as a save failure (the same
/// verdict the store-backed path gives the identical physical state).
#[test]
fn save_done_ok_with_durable_false_succeeds_and_warns() {
    let mut app = test_app();
    let id = app.active;
    let version = app.doc(id).unwrap().buffer.version();

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Ok(()),
            durable: false,
        },
        &mut effects,
    );

    let log = rune_tui::messages::log_text(&app);
    assert!(
        log.contains("durability unconfirmed"),
        "an unconfirmed-durability save must warn: {log:?}"
    );
    assert!(
        !log.contains("save failed"),
        "physical success must never be reported as a failure: {log:?}"
    );
}

/// The log is append-only: a save failure's entry stays in the log even
/// after a LATER save on the same document succeeds — a success posts
/// nothing at all, so there is nothing to clear.
#[test]
fn save_done_ok_advances_saved_version_and_keeps_a_prior_save_failure_in_the_log() {
    let mut app = test_app();
    let id = app.active;
    let version = app.doc(id).unwrap().buffer.version();

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Err("oops".to_string()),
            durable: true,
        },
        &mut effects,
    );
    assert!(rune_tui::messages::newest_text(&app).is_some());

    let mut effects2 = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Ok(()),
            durable: true,
        },
        &mut effects2,
    );
    assert_eq!(app.doc(id).unwrap().saved_version, version);
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|s| s.contains("oops")),
        "a successful save must never clear the failure message an earlier \
         save left in the log"
    );
}

/// A successful save must not disturb an unrelated log entry some OTHER
/// subsystem posted — e.g. an edit/undo/redo failure. Both entries stay in
/// the log, in order — the log is append-only, so there is no
/// provenance question to resolve.
#[test]
fn save_done_ok_keeps_an_unrelated_log_entry() {
    let mut app = test_app();
    let id = app.active;
    rune_tui::messages::warn(&mut app, "edit failed: some other message");
    assert!(rune_tui::messages::newest_text(&app).is_some());

    let version = app.doc(id).unwrap().buffer.version();
    let mut effects2 = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Ok(()),
            durable: true,
        },
        &mut effects2,
    );

    assert_eq!(app.doc(id).unwrap().saved_version, version);
    assert_eq!(
        rune_tui::messages::log_text(&app),
        "edit failed: some other message",
        "a successful save must not clear or add to an unrelated log entry"
    );
}

#[test]
fn save_done_err_surfaces_status_and_keeps_dirty() {
    let mut app = test_app();
    let id = app.active;
    app.doc_mut(id).unwrap().buffer = app
        .doc(id)
        .unwrap()
        .buffer
        .insert(0, "x")
        .expect("in-bounds insert should apply");
    let before_saved = app.doc(id).unwrap().saved_version;
    let version = app.doc(id).unwrap().buffer.version();
    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Err("disk full".to_string()),
            durable: true,
        },
        &mut effects,
    );
    assert_eq!(app.doc(id).unwrap().saved_version, before_saved);
    assert!(app.is_dirty());
    assert!(rune_tui::messages::newest_text(&app).is_some_and(|s| s.contains("disk full")));
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
        // Built EMPTY, then the fixture's exact text is typed in via the
        // real insert path (`dirty_common`'s doc comment explains why: a
        // document's `saved_content` baseline is seeded from the buffer at
        // construction, so starting AT `content` can never be dirty — only
        // an edit AWAY from the constructed baseline can be, and it must
        // land on exactly `content` for this test's byte-exact assertion
        // below to mean anything).
        let mut app = App::new(
            Buffer::new(""),
            Some(path.clone()),
            Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
            None,
        );
        let id = app.active;
        edit::insert_text(&mut app, id, content);

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
    dirty_common::force_dirty(&mut app, id);

    let effects = press_save(&mut app);
    settle_cmds(&mut app, effects);

    assert!(app.is_dirty());
    assert!(
        rune_tui::messages::newest_text(&app).is_some(),
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
    app.doc_mut(id).unwrap().buffer = app
        .doc(id)
        .unwrap()
        .buffer
        .insert(0, "x")
        .expect("in-bounds insert should apply"); // makes it dirty

    let effects = press_save(&mut app);
    assert_eq!(effects.cmds.len(), 1);
    assert!(app.doc(id).unwrap().save_in_flight);

    let effects2 = press_save(&mut app);
    assert!(
        !spawns_a_save(&effects2),
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
    dirty_common::force_dirty(&mut app, id);

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
    // Built EMPTY, then typed in via the real insert path — see the sibling
    // CRLF/BOM test above for why: only an edit AWAY from the constructed
    // baseline is dirty, and it must land on the exact target bytes for the
    // byte-exact assertion below.
    let mut app = App::new(Buffer::new(""), Some(path.clone()), vfs, None);
    let id = app.active;
    edit::insert_text(&mut app, id, "brand new file\n");

    let effects = press_save(&mut app);
    settle_cmds(&mut app, effects);

    assert!(!app.is_dirty());
    let saved = std::fs::read(&path).expect("save must have created the file on disk");
    assert_eq!(saved, b"brand new file\n");

    let _ = std::fs::remove_file(&path);
}

/// Regression: `trigger_save`'s pathless arm must never touch
/// `display_name` — that field is only ever cleared once a path is
/// actually bound (`Document::bind_path`). A dirty untitled document (the
/// default no-arg launch's own shape: `file_path: None`, `display_name:
/// Some("Untitled 1")`) that gets ⌘S pressed on it has no path to save to,
/// so the save is refused — but the title must still read "Untitled 1"
/// afterward, not silently flip to the `"[No Name]"` placeholder.
#[test]
fn save_on_a_dirty_untitled_document_leaves_the_title_unchanged() {
    let mut app = test_app();
    let id = app.active;
    app.doc_mut(id).unwrap().display_name = Some("Untitled 1".to_string());
    edit::insert_char(&mut app, id, '!');
    assert!(app.is_dirty(), "the fixture must actually be dirty");

    press_save(&mut app);

    assert_eq!(
        app.doc(id).unwrap().file_name(),
        "Untitled 1",
        "a refused pathless save must never clear display_name"
    );
}

/// The highest-value regression in the package: ⌘S on a `Preview`
/// document must never reach `vfs.save_atomic` — every global save chord
/// routes to `trigger_save` unconditionally, and the no-store fallback
/// there would otherwise atomically overwrite the previewed file with
/// this document's own (edited) buffer, a data-safety violation. The
/// document is dirtied FIRST, while still `ReadOnly::No` (a preview has no
/// production path to become dirty, since the edit chokepoint already
/// refuses any read-only document), then flipped to `Preview` — the same
/// sequence `reading_view_blocks_undo_and_redo` (`tests/edit_commands.rs`)
/// uses for the identical reason.
#[test]
fn preview_document_refuses_save_and_never_touches_disk() {
    let vfs = Arc::new(Mem::new());
    let path = PathBuf::from("/doc.md");
    vfs.save_atomic(&path, b"on disk").expect("seed doc.md");
    let mut app = App::new(
        Buffer::new("on disk"),
        Some(path.clone()),
        Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
        None,
    );
    let id = app.active;
    edit::insert_text(&mut app, id, "!");
    assert!(app.is_dirty(), "the fixture must actually be dirty");

    app.doc_mut(id).unwrap().read_only = rune_tui::document::ReadOnly::Preview;

    let effects = press_save(&mut app);
    assert!(
        !spawns_a_save(&effects),
        "a refused preview save must spawn no save Cmd"
    );

    let saved = vfs.read(&path).expect("the seeded file must still exist");
    assert_eq!(
        saved, b"on disk",
        "a preview save must never touch disk, dirty buffer or not"
    );
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        rune_tui::document::ReadOnly::Preview.refusal_message()
    );
}
