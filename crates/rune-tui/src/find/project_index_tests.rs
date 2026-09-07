use std::path::{Path, PathBuf};

use rune_vfs::VfsTestExt;

use super::test_support::*;
use crate::projectsearch::index::{IndexEntry, MAX_INDEX_FILE_BYTES, READ_BATCH, ReadOutcome};
use crate::runtime::{CmdKind, Effects, Msg, TimerKey, TimerMsgKey};

fn indexed_paths(app: &crate::app::App) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = app
        .project_index
        .as_ref()
        .expect("index exists")
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    paths.sort();
    paths
}

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

fn build_generation(app: &crate::app::App) -> crate::generation::ProjectIndexGen {
    app.project_index
        .as_ref()
        .expect("open built the index state")
        .build_generation
}

#[test]
fn opening_in_project_scope_scans_then_batch_reads_exactly_the_indexable_files() {
    let over_cap = vec![b'x'; MAX_INDEX_FILE_BYTES as usize + 1];
    let mut app = seeded_app(&[
        ("/root/notes.md", b"Hello World"),
        ("/root/sub/keep.txt", b"plain text"),
        ("/root/.gitignore", b"secret.md\n"),
        ("/root/secret.md", b"gitignored"),
        ("/root/.dockerignore", b"docker.md\n"),
        ("/root/docker.md", b"dockerignored"),
        ("/root/node_modules/dep.js", b"dependency"),
        ("/root/__pycache__/mod.pyc", b"bytecode"),
        ("/root/song.mp3", b"not audio but named so"),
        ("/root/nul.txt", b"has a \0 byte"),
        ("/root/huge.txt", over_cap.as_slice()),
    ]);
    let mut effects = open_project_find(&mut app);

    let scanned = run_one_cmd(&mut effects, CmdKind::ProjectIndex).expect("scan cmd dispatched");
    assert!(
        matches!(scanned, Msg::ProjectIndexScanned { .. }),
        "the first reply is the walk, not a read batch"
    );
    crate::app::update(&mut app, scanned, &mut effects);
    let batch =
        run_one_cmd(&mut effects, CmdKind::ProjectIndex).expect("a read batch follows the scan");
    assert!(matches!(batch, Msg::ProjectIndexBatch { .. }));
    crate::app::update(&mut app, batch, &mut effects);
    pump_index(&mut app, &mut effects);

    assert_eq!(
        indexed_paths(&app),
        vec![
            PathBuf::from("/root/notes.md"),
            PathBuf::from("/root/sub/keep.txt"),
        ]
    );
    let index = app.project_index.as_ref().expect("index exists");
    assert!(!index.building, "the build completed");
    assert!(!index.truncated);
    let entry = index
        .entries
        .iter()
        .find(|e| e.path == Path::new("/root/notes.md"))
        .expect("notes.md indexed");
    assert_eq!(entry.text, "Hello World");
    assert_eq!(entry.display, "notes.md");
}

#[test]
fn reading_stops_past_the_corpus_cap_and_marks_the_index_truncated() {
    let seeds: Vec<(String, Vec<u8>)> = (0..READ_BATCH + 1)
        .map(|i| (format!("/root/f{i:03}.txt"), b"body".to_vec()))
        .collect();
    let refs: Vec<(&str, &[u8])> = seeds
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_slice()))
        .collect();
    let mut app = seeded_app(&refs);
    let mut effects = open_project_find(&mut app);
    app.project_index
        .as_mut()
        .expect("open built the index state")
        .corpus_cap = 4;

    pump_index(&mut app, &mut effects);

    let index = app.project_index.as_ref().expect("index exists");
    assert_eq!(
        index.entries.len(),
        READ_BATCH,
        "the batch that crossed the cap is kept; the next one never dispatches"
    );
    assert!(index.truncated);
    assert!(!index.building);
    assert!(index.pending.is_empty());
}

#[test]
fn a_stale_generation_batch_is_dropped() {
    let mut app = seeded_app(&[("/root/notes.md", b"hello")]);
    let mut effects = open_project_find(&mut app);
    let live = build_generation(&app);

    let stale = crate::generation::ProjectIndexGen::from_raw(live.raw() + 1);
    let entry = IndexEntry {
        path: PathBuf::from("/root/ghost.md"),
        display: "ghost.md".to_string(),
        text: "ghost".to_string(),
        size: 5,
        mtime: std::time::SystemTime::UNIX_EPOCH,
    };
    crate::app::update(
        &mut app,
        Msg::ProjectIndexBatch {
            generation: stale,
            outcomes: vec![ReadOutcome::Indexed(entry)],
        },
        &mut effects,
    );

    let index = app.project_index.as_ref().expect("index exists");
    assert!(
        index.entries.is_empty(),
        "a stale batch must never reach the corpus"
    );
}

#[test]
fn a_spinner_tick_while_building_advances_the_frame_and_rearms() {
    let mut app = seeded_app(&[("/root/notes.md", b"hello")]);
    let mut effects = open_project_find(&mut app);
    let spinner_key = TimerKey::from(TimerMsgKey::ProjectSearchSpinner);
    let armed_at_open = app
        .timers
        .armed_deadline(spinner_key)
        .expect("build start arms the spinner");
    let generation = build_generation(&app);

    crate::app::update(
        &mut app,
        Msg::Timer {
            key: TimerMsgKey::ProjectSearchSpinner,
            generation: generation.raw(),
        },
        &mut effects,
    );

    let index = app.project_index.as_ref().expect("index exists");
    assert_eq!(index.spinner_frame, 1);
    let rearmed = app
        .timers
        .armed_deadline(spinner_key)
        .expect("still armed while building");
    assert!(rearmed > armed_at_open, "the tick pushed a fresh deadline");
    let readout = project_readout_row(&mut app);
    assert!(
        readout
            .chars()
            .any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
        "the readout shows the spinner while the build is in flight: {readout:?}"
    );
}

#[test]
fn after_the_build_completes_a_tick_does_not_rearm() {
    let mut app = seeded_app(&[("/root/notes.md", b"hello")]);
    let mut effects = open_project_find(&mut app);
    let generation = build_generation(&app);
    pump_index(&mut app, &mut effects);
    assert!(
        !app.project_index.as_ref().expect("index exists").building,
        "test setup: build finished"
    );
    let spinner_key = TimerKey::from(TimerMsgKey::ProjectSearchSpinner);
    let before = app.timers.armed_deadline(spinner_key);

    crate::app::update(
        &mut app,
        Msg::Timer {
            key: TimerMsgKey::ProjectSearchSpinner,
            generation: generation.raw(),
        },
        &mut effects,
    );

    let index = app.project_index.as_ref().expect("index exists");
    assert_eq!(index.spinner_frame, 0, "a dead build never animates");
    assert_eq!(
        app.timers.armed_deadline(spinner_key),
        before,
        "no rearm after completion"
    );
}

#[test]
fn reopening_after_an_edit_reindexes_the_new_content() {
    let mut app = seeded_app(&[("/root/notes.md", b"old words")]);
    let mut effects = open_project_find(&mut app);
    pump_index(&mut app, &mut effects);
    press(&mut app, escape());
    app.vfs
        .save_atomic(Path::new("/root/notes.md"), b"Fresh words")
        .expect("edit the file on disk");

    let mut effects = open_project_find(&mut app);
    pump_index(&mut app, &mut effects);

    let index = app.project_index.as_ref().expect("index exists");
    assert_eq!(
        index.entries.len(),
        1,
        "the edit replaces, never duplicates"
    );
    let entry = index.entries.first().expect("one entry");
    assert_eq!(entry.text, "Fresh words");
    assert_eq!(
        index.corpus_bytes,
        entry.text.len(),
        "replacing an entry keeps the corpus accounting exact"
    );
}

#[test]
fn a_deleted_file_disappears_from_results_after_the_rescan() {
    let mut app = seeded_app(&[
        ("/root/a.md", b"needle alpha"),
        ("/root/b.md", b"needle beta"),
    ]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    assert_eq!(
        result_displays(&app).len(),
        2,
        "test setup: both files match before the deletion"
    );
    press(&mut app, escape());
    app.vfs
        .remove(Path::new("/root/b.md"))
        .expect("delete the file");

    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);

    assert_eq!(result_displays(&app), vec!["a.md".to_string()]);
    assert_eq!(indexed_displays(&app), vec!["a.md".to_string()]);
}

#[test]
fn a_truncated_rescan_drops_no_entries() {
    let mut app = seeded_app(&[("/root/a.md", b"alpha"), ("/root/b.md", b"beta")]);
    let mut effects = open_project_find(&mut app);
    pump_index(&mut app, &mut effects);
    press(&mut app, escape());

    let mut effects = open_project_find(&mut app);
    let generation = build_generation(&app);
    let real_scan = run_one_cmd(&mut effects, CmdKind::ProjectIndex);
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
    let mut effects = open_project_find(&mut app);
    pump_index(&mut app, &mut effects);
    press(&mut app, escape());

    let mut effects = open_project_find(&mut app);
    let scanned = run_one_cmd(&mut effects, CmdKind::ProjectIndex).expect("rescan dispatched");
    crate::app::update(&mut app, scanned, &mut effects);
    let batch =
        run_one_cmd(&mut effects, CmdKind::ProjectIndex).expect("a read batch follows the rescan");

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
    let mut effects = open_project_find(&mut app);
    pump_index(&mut app, &mut effects);
    press(&mut app, escape());
    app.root = Some(PathBuf::from("/other"));

    let mut effects = open_project_find(&mut app);
    pump_index(&mut app, &mut effects);

    let index = app.project_index.as_ref().expect("index exists");
    assert_eq!(index.root, Path::new("/other"));
    assert_eq!(indexed_displays(&app), vec!["b.md".to_string()]);
}

#[test]
fn switching_the_scope_chip_to_project_builds_the_index_and_arms_the_debounce() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);
    let mut effects = Effects::default();
    press_into(
        &mut app,
        key(crate::keymap::KeyCode::Char('f'), CTRL),
        &mut effects,
    );
    type_into(&mut app, "needle", &mut effects);
    assert!(app.project_index.is_none(), "File scope builds no index");

    press_into(&mut app, tab(), &mut effects);
    press_into(&mut app, space(), &mut effects);

    assert_eq!(find(&app).scope(), crate::find::Scope::Project);
    assert!(
        app.project_index.is_some(),
        "entering Project scope starts the index build"
    );
    assert!(
        app.timers
            .armed_deadline(TimerKey::from(TimerMsgKey::ProjectSearchDebounce))
            .is_some(),
        "the existing query is re-run against the project after the debounce"
    );
    pump_index(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);
    assert_eq!(result_displays(&app), vec!["a.md".to_string()]);
}
