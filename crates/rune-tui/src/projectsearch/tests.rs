#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, VfsTestExt};

use crate::app::App;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::pane::Pane;
use crate::runtime::{CmdKind, Effects, Msg, TimerKey, TimerMsgKey};

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};
const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};

fn app() -> App {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    app.frame = Some(crate::app::FrameSize::new(120, 34));
    app
}

fn key(app: &mut App, code: KeyCode, mods: Mods, effects: &mut Effects) {
    crate::app::update(app, Msg::Key(KeyInput { code, mods }), effects);
}

#[test]
fn ctrl_shift_f_chord_opens_project_search_on_a_never_shown_left_column() {
    let mut app = app();
    assert!(!app.splits.left.is_shown(), "test setup: column hidden");
    let mut effects = Effects::default();

    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);

    assert!(app.projectsearch().is_some());
    assert_eq!(app.focus(), Pane::Explorer);
}

#[test]
fn sup_shift_f_chord_again_closes_and_restores_return_to() {
    let mut app = app();
    let second = app.open_document(Buffer::new("second"));
    crate::workspace::switch_to(&mut app, second);
    let mut effects = Effects::default();

    key(&mut app, KeyCode::Char('F'), SUP, &mut effects);
    assert!(app.projectsearch().is_some(), "test setup: panel open");

    key(&mut app, KeyCode::Char('F'), SUP, &mut effects);

    assert!(app.projectsearch().is_none());
    assert_eq!(app.active, second);
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn escape_closes_and_restores_return_to() {
    let mut app = app();
    let second = app.open_document(Buffer::new("second"));
    crate::workspace::switch_to(&mut app, second);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    assert!(app.projectsearch().is_some(), "test setup: panel open");

    key(&mut app, KeyCode::Escape, Mods::NONE, &mut effects);

    assert!(app.projectsearch().is_none());
    assert_eq!(app.active, second);
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn typed_chars_echo_in_the_query_without_reminting_the_generation() {
    let mut app = app();
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    let minted = app.projectsearch().expect("open").query_generation;

    key(&mut app, KeyCode::Char('h'), Mods::NONE, &mut effects);
    key(&mut app, KeyCode::Char('i'), Mods::NONE, &mut effects);
    key(&mut app, KeyCode::Backspace, Mods::NONE, &mut effects);

    let state = app.projectsearch().expect("still open");
    assert_eq!(state.query, "h");
    assert_eq!(state.query_generation, minted);
    assert_eq!(
        app.active_doc().buffer.content(),
        "hello",
        "typing into the panel must never reach the editor"
    );
}

#[test]
fn opening_over_the_file_finder_tears_it_down_through_cancel() {
    let mut app = app();
    let second = app.open_document(Buffer::new("second"));
    crate::workspace::switch_to(&mut app, second);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('o'), SUP, &mut effects);
    assert!(app.filesearch().is_some(), "test setup: finder open");

    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);

    assert!(app.filesearch().is_none());
    assert!(app.projectsearch().is_some());
    assert_eq!(
        app.active, second,
        "the finder's return_to must be restored before the panel records its own"
    );
    assert_eq!(app.focus(), Pane::Explorer);
}

#[test]
fn a_close_bars_global_closes_project_search_through_close_all_overlays() {
    let mut app = app();
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    assert!(app.projectsearch().is_some(), "test setup: panel open");

    key(&mut app, KeyCode::F1, Mods::NONE, &mut effects);

    assert!(app.projectsearch().is_none());
}

#[test]
fn a_paste_lands_in_the_query_not_the_editor() {
    let mut app = app();
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);

    crate::app::update(
        &mut app,
        Msg::Paste("grep\nsecond line".to_string()),
        &mut effects,
    );

    assert_eq!(
        app.projectsearch().map(|s| s.query.as_str()),
        Some("grep"),
        "only the first pasted line survives sanitization"
    );
    assert_eq!(app.active_doc().buffer.content(), "hello");
}

fn seeded_app(files: &[(&str, &[u8])]) -> App {
    let vfs = Mem::new();
    for (path, content) in files {
        vfs.save_atomic(Path::new(path), content)
            .expect("seed file");
    }
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(vfs), None);
    app.frame = Some(crate::app::FrameSize::new(120, 34));
    app.root = Some(PathBuf::from("/root"));
    app
}

fn run_one_index_cmd(effects: &mut Effects) -> Option<Msg> {
    let position = effects
        .cmds
        .iter()
        .position(|cmd| cmd.kind() == CmdKind::ProjectIndex)?;
    effects.cmds.remove(position).run()
}

fn pump_index(app: &mut App, effects: &mut Effects) {
    while let Some(msg) = run_one_index_cmd(effects) {
        crate::app::update(app, msg, effects);
    }
}

fn indexed_paths(app: &App) -> Vec<PathBuf> {
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

#[test]
fn opening_scans_then_batch_reads_exactly_the_indexable_files() {
    let over_cap = vec![b'x'; super::index::MAX_INDEX_FILE_BYTES as usize + 1];
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
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);

    let scanned = run_one_index_cmd(&mut effects).expect("scan cmd dispatched");
    assert!(
        matches!(scanned, Msg::ProjectIndexScanned { .. }),
        "the first reply is the walk, not a read batch"
    );
    crate::app::update(&mut app, scanned, &mut effects);
    let batch = run_one_index_cmd(&mut effects).expect("a read batch follows the scan");
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
    assert_eq!(entry.folded, "hello world");
    assert_eq!(entry.display, "notes.md");
}

#[test]
fn reading_stops_past_the_corpus_cap_and_marks_the_index_truncated() {
    let seeds: Vec<(String, Vec<u8>)> = (0..super::index::READ_BATCH + 1)
        .map(|i| (format!("/root/f{i:03}.txt"), b"body".to_vec()))
        .collect();
    let refs: Vec<(&str, &[u8])> = seeds
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_slice()))
        .collect();
    let mut app = seeded_app(&refs);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    app.project_index
        .as_mut()
        .expect("open built the index state")
        .corpus_cap = 4;

    pump_index(&mut app, &mut effects);

    let index = app.project_index.as_ref().expect("index exists");
    assert_eq!(
        index.entries.len(),
        super::index::READ_BATCH,
        "the batch that crossed the cap is kept; the next one never dispatches"
    );
    assert!(index.truncated);
    assert!(!index.building);
    assert!(index.pending.is_empty());
}

#[test]
fn a_stale_generation_batch_is_dropped() {
    let mut app = seeded_app(&[("/root/notes.md", b"hello")]);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    let live = app
        .project_index
        .as_ref()
        .expect("open built the index state")
        .build_generation;

    let stale = crate::generation::ProjectIndexGen::from_raw(live.raw() + 1);
    let entry = super::index::IndexEntry {
        path: PathBuf::from("/root/ghost.md"),
        display: "ghost.md".to_string(),
        text: "ghost".to_string(),
        folded: "ghost".to_string(),
        size: 5,
        mtime: std::time::SystemTime::UNIX_EPOCH,
    };
    crate::app::update(
        &mut app,
        Msg::ProjectIndexBatch {
            generation: stale,
            outcomes: vec![super::index::ReadOutcome::Indexed(entry)],
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
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    let spinner_key = TimerKey::from(TimerMsgKey::ProjectSearchSpinner);
    let armed_at_open = app
        .timers
        .armed_deadline(spinner_key)
        .expect("build start arms the spinner");
    let generation = app
        .project_index
        .as_ref()
        .expect("open built the index state")
        .build_generation;

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
}

#[test]
fn after_the_build_completes_a_tick_does_not_rearm() {
    let mut app = seeded_app(&[("/root/notes.md", b"hello")]);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    let generation = app
        .project_index
        .as_ref()
        .expect("open built the index state")
        .build_generation;
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
