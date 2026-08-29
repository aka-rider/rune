#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use rune_vfs::VfsTestExt;

use crate::keymap::{KeyCode, Mods};
use crate::runtime::{CmdKind, Effects, Msg, TimerMsgKey};

use super::index::ReadOutcome;
use super::preview_tests::run_one_cmd;
use super::tests::{CTRL, key, pump_index, run_one_index_cmd, seeded_app};

fn indexed_displays(app: &crate::app::App) -> Vec<String> {
    let mut displays: Vec<String> = app
        .project_index
        .as_ref()
        .expect("index exists")
        .entries
        .iter()
        .map(|entry| entry.display.clone())
        .collect();
    displays.sort();
    displays
}

#[test]
fn reopening_after_an_edit_reindexes_the_new_content() {
    let mut app = seeded_app(&[("/root/notes.md", b"old words")]);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    pump_index(&mut app, &mut effects);
    key(&mut app, KeyCode::Escape, Mods::NONE, &mut effects);
    app.vfs
        .save_atomic(Path::new("/root/notes.md"), b"Fresh words")
        .expect("edit the file on disk");

    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    pump_index(&mut app, &mut effects);

    let index = app.project_index.as_ref().expect("index exists");
    assert_eq!(
        index.entries.len(),
        1,
        "the edit replaces, never duplicates"
    );
    let entry = index.entries.first().expect("one entry");
    assert_eq!(entry.text, "Fresh words");
    assert_eq!(entry.folded, "fresh words");
    assert_eq!(
        index.corpus_bytes,
        entry.text.len() + entry.folded.len(),
        "replacing an entry keeps the corpus accounting exact"
    );
}

#[test]
fn a_deleted_file_disappears_from_results_after_reopen() {
    let mut app = seeded_app(&[
        ("/root/a.md", b"needle alpha"),
        ("/root/b.md", b"needle beta"),
    ]);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    pump_index(&mut app, &mut effects);
    for c in "needle".chars() {
        key(&mut app, KeyCode::Char(c), Mods::NONE, &mut effects);
    }
    crate::app::update(
        &mut app,
        Msg::Timer {
            key: TimerMsgKey::ProjectSearchDebounce,
            generation: 0,
        },
        &mut effects,
    );
    let reply = run_one_cmd(&mut effects, CmdKind::ProjectQuery).expect("query cmd dispatched");
    crate::app::update(&mut app, reply, &mut effects);
    assert_eq!(
        app.projectsearch().expect("panel open").results.len(),
        2,
        "test setup: both files match before the deletion"
    );
    key(&mut app, KeyCode::Escape, Mods::NONE, &mut effects);
    app.vfs
        .remove(Path::new("/root/b.md"))
        .expect("delete the file");

    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    let immediate =
        run_one_cmd(&mut effects, CmdKind::ProjectQuery).expect("reopen reruns the last query");
    crate::app::update(&mut app, immediate, &mut effects);
    pump_index(&mut app, &mut effects);
    let rerun =
        run_one_cmd(&mut effects, CmdKind::ProjectQuery).expect("refresh completion reruns");
    crate::app::update(&mut app, rerun, &mut effects);

    let displays: Vec<String> = app
        .projectsearch()
        .expect("panel open")
        .results
        .iter()
        .map(|hit| hit.display.clone())
        .collect();
    assert_eq!(displays, vec!["a.md".to_string()]);
    assert_eq!(indexed_displays(&app), vec!["a.md".to_string()]);
}

#[test]
fn a_truncated_rescan_drops_no_entries() {
    let mut app = seeded_app(&[("/root/a.md", b"alpha"), ("/root/b.md", b"beta")]);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    pump_index(&mut app, &mut effects);
    key(&mut app, KeyCode::Escape, Mods::NONE, &mut effects);

    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    let generation = app
        .project_index
        .as_ref()
        .expect("index exists")
        .build_generation;
    let real_scan = run_one_index_cmd(&mut effects);
    assert!(
        matches!(real_scan, Some(Msg::ProjectIndexScanned { .. })),
        "test setup: the reopen dispatched a rescan"
    );
    crate::app::update(
        &mut app,
        Msg::ProjectIndexScanned {
            generation,
            result: Ok(crate::filesearch::walk::ScanResult {
                files: vec![PathBuf::from("/root/a.md")],
                truncated: true,
            }),
        },
        &mut effects,
    );
    pump_index(&mut app, &mut effects);

    let index = app.project_index.as_ref().expect("index exists");
    assert!(index.truncated);
    assert_eq!(
        indexed_displays(&app),
        vec!["a.md".to_string(), "b.md".to_string()],
        "a truncated scan is an arbitrary prefix; absence proves nothing"
    );
}

#[test]
fn an_unchanged_file_reads_back_as_unchanged() {
    let mut app = seeded_app(&[("/root/notes.md", b"stable")]);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    pump_index(&mut app, &mut effects);
    key(&mut app, KeyCode::Escape, Mods::NONE, &mut effects);

    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    let scanned = run_one_index_cmd(&mut effects).expect("rescan dispatched");
    crate::app::update(&mut app, scanned, &mut effects);
    let batch = run_one_index_cmd(&mut effects).expect("a read batch follows the rescan");

    assert!(matches!(&batch, Msg::ProjectIndexBatch { .. }));
    if let Msg::ProjectIndexBatch { outcomes, .. } = &batch {
        assert!(
            matches!(
                outcomes.as_slice(),
                [ReadOutcome::Unchanged(path)] if path == Path::new("/root/notes.md")
            ),
            "an unmodified file must not be re-read, got {outcomes:?}"
        );
    }
    crate::app::update(&mut app, batch, &mut effects);
    pump_index(&mut app, &mut effects);
    let index = app.project_index.as_ref().expect("index exists");
    assert_eq!(index.entries.len(), 1);
    assert_eq!(
        index.entries.first().expect("one entry").text,
        "stable",
        "Unchanged retains the existing entry"
    );
}

#[test]
fn reopening_under_a_different_root_discards_and_cold_builds() {
    let mut app = seeded_app(&[("/root/a.md", b"alpha"), ("/other/b.md", b"beta")]);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    pump_index(&mut app, &mut effects);
    key(&mut app, KeyCode::Escape, Mods::NONE, &mut effects);
    app.root = Some(PathBuf::from("/other"));

    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    pump_index(&mut app, &mut effects);

    let index = app.project_index.as_ref().expect("index exists");
    assert_eq!(index.root, Path::new("/other"));
    assert_eq!(indexed_displays(&app), vec!["b.md".to_string()]);
}
