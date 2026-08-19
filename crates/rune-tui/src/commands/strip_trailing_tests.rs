#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::{Cursor, CursorSet};
use rune_vfs::{Mem, Vfs, VfsTestExt};

use super::*;
use crate::commands::edit;
use crate::document::ReadOnly;
use crate::guard::{GuardKind, GuardPrompt, set_guard};
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::merge::session::{
    Block, BlockOrigin, Conflict, ConflictBlock, MergeSession, Resolution,
};
use crate::merge::state::MergeState;
use crate::runtime::{CmdKind, Effects, Msg};
use crate::save::{SaveMode, SaveOrigin, SaveStart, trigger_save};

const DOC_PATH: &str = "/doc.md";

fn app_with(content: &str) -> (App, Arc<Mem>, DocumentId) {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new(DOC_PATH), content.as_bytes())
        .expect("seed the document on disk");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let app = App::new(
        Buffer::new(content),
        Some(PathBuf::from(DOC_PATH)),
        vfs,
        None,
    );
    let id = app.active;
    (app, mem, id)
}

fn caret_at(app: &mut App, id: DocumentId, offset: usize) {
    app.doc_mut(id).unwrap().cursors = CursorSet::new(offset);
}

fn select(app: &mut App, id: DocumentId, anchor: usize, position: usize) {
    let primary = app.doc(id).unwrap().cursors.primary();
    app.doc_mut(id).unwrap().cursors = CursorSet::new_from(&[Cursor {
        anchor,
        position,
        ..primary
    }]);
}

fn save_interactively(app: &mut App, id: DocumentId) -> (SaveStart, Effects) {
    let mut effects = Effects::default();
    let start = trigger_save(
        app,
        id,
        SaveMode::Normal,
        SaveOrigin::Interactive,
        &mut effects,
    );
    (start, effects)
}

fn run_the_save_cmd(app: &mut App, effects: &mut Effects) {
    let cmd = effects
        .cmds
        .drain(..)
        .find(|cmd| cmd.kind() == CmdKind::Save)
        .expect("a started save pushes its publish Cmd");
    let reply = cmd.run().expect("the publish Cmd replies");
    assert!(matches!(reply, Msg::SaveDone { .. }));
    crate::app::update(app, reply, effects);
}

fn content(app: &App, id: DocumentId) -> String {
    app.doc(id).unwrap().buffer.content().to_string()
}

fn resolved_merge_session(app: &mut App, id: DocumentId) {
    app.merge = MergeState::Active {
        doc: id,
        session: MergeSession {
            conflicts: vec![ConflictBlock {
                conflict: Conflict {
                    ours: "ours".to_string(),
                    theirs: "theirs".to_string(),
                },
                block: Block {
                    range: 0..1,
                    resolution: Resolution::KeptOurs,
                    origin: BlockOrigin::Conflict,
                },
            }],
            cur: 0,
            saved_display_name: None,
            theirs_obs: rune_db::ObsId::new(1).expect("nonzero"),
            install_pos: 0,
        },
    };
}

#[test]
fn a_caret_past_one_stripped_run_moves_back_by_only_that_run() {
    let (mut app, _mem, id) = app_with("a  \nb  \nc");
    caret_at(&mut app, id, 5);

    let (start, _effects) = save_interactively(&mut app, id);

    assert_eq!(start, SaveStart::InFlight);
    assert_eq!(content(&app, id), "a\nb\nc");
    assert_eq!(app.doc(id).unwrap().cursors.primary().position, 3);
}

#[test]
fn a_caret_inside_a_stripped_run_lands_where_the_run_began() {
    let (mut app, _mem, id) = app_with("a  \nb  \nc");
    caret_at(&mut app, id, 6);

    let (_start, _effects) = save_interactively(&mut app, id);

    assert_eq!(app.doc(id).unwrap().cursors.primary().position, 3);
}

#[test]
fn a_selection_spanning_stripped_lines_keeps_both_of_its_ends() {
    let (mut app, _mem, id) = app_with("alpha  \nbeta  \ngamma");
    select(&mut app, id, 2, 17);

    let (_start, _effects) = save_interactively(&mut app, id);

    assert_eq!(content(&app, id), "alpha\nbeta\ngamma");
    let primary = app.doc(id).unwrap().cursors.primary();
    assert_eq!(primary.anchor, 2);
    assert_eq!(primary.position, 13);
}

#[test]
fn one_undo_after_a_stripping_save_restores_every_byte_and_leaves_the_document_dirty() {
    let (mut app, _mem, id) = app_with("a  \nb\t\nc");

    let (_start, mut effects) = save_interactively(&mut app, id);
    assert_eq!(content(&app, id), "a\nb\nc");
    run_the_save_cmd(&mut app, &mut effects);

    edit::undo(&mut app, id);

    assert_eq!(content(&app, id), "a  \nb\t\nc");
    assert!(app.doc(id).unwrap().is_dirty());
}

#[test]
fn saving_a_document_with_nothing_to_strip_preserves_the_redo_tail() {
    let (mut app, _mem, id) = app_with("clean\n");
    edit::insert_char(&mut app, id, 'X');
    edit::undo(&mut app, id);
    let version_before = app.doc(id).unwrap().buffer.version();

    let (start, _effects) = save_interactively(&mut app, id);

    assert_eq!(start, SaveStart::NotDirty);
    assert_eq!(app.doc(id).unwrap().buffer.version(), version_before);

    edit::redo(&mut app, id);

    assert_eq!(content(&app, id), "Xclean\n");
}

#[test]
fn a_document_clean_on_open_but_carrying_trailing_whitespace_is_cleaned_by_a_plain_save() {
    let (mut app, mem, id) = app_with("messy   \nlines\t\n");
    assert!(!app.doc(id).unwrap().is_dirty());

    let (start, mut effects) = save_interactively(&mut app, id);

    assert_eq!(start, SaveStart::InFlight);
    assert_eq!(content(&app, id), "messy\nlines\n");
    run_the_save_cmd(&mut app, &mut effects);
    assert_eq!(
        mem.read(Path::new(DOC_PATH)).expect("read back"),
        b"messy\nlines\n"
    );
}

#[test]
fn a_fully_resolved_merge_session_saves_verbatim_and_relabels_no_block() {
    let (mut app, _mem, id) = app_with("kept  \n");
    edit::insert_char(&mut app, id, 'X');
    resolved_merge_session(&mut app, id);

    let (start, _effects) = save_interactively(&mut app, id);

    assert_eq!(start, SaveStart::InFlight);
    assert_eq!(content(&app, id), "Xkept  \n");
    let resolutions: Vec<Resolution> = match &app.merge {
        MergeState::Active { session, .. } => session
            .conflicts
            .iter()
            .map(|block| block.block.resolution)
            .collect(),
        _ => Vec::new(),
    };
    assert_eq!(resolutions, vec![Resolution::KeptOurs]);
}

#[test]
fn an_interactive_save_in_reading_view_is_refused_and_says_why() {
    let (mut app, _mem, id) = app_with("edited  \n");
    edit::insert_char(&mut app, id, 'X');
    app.doc_mut(id).unwrap().read_only = ReadOnly::Reading;

    let (start, _effects) = save_interactively(&mut app, id);

    assert_eq!(start, SaveStart::Refused);
    assert_eq!(content(&app, id), "Xedited  \n");
    assert_eq!(
        crate::messages::newest_text(&app),
        ReadOnly::Reading.refusal_message()
    );
}

#[test]
fn the_dirty_quit_guard_saves_a_reading_view_document_stripped_and_whole() {
    let (mut app, mem, id) = app_with("edited  \n");
    edit::insert_char(&mut app, id, 'X');
    app.doc_mut(id).unwrap().read_only = ReadOnly::Reading;
    let _ = set_guard(
        &mut app,
        GuardPrompt {
            doc: id,
            kind: GuardKind::DirtyQuit,
        },
        &mut crate::runtime::Effects::default(),
    );

    let mut effects = Effects::default();
    crate::guard::handle_guard_key(
        &mut app,
        KeyInput {
            code: KeyCode::Char('s'),
            mods: Mods::NONE,
        },
        &mut effects,
    );

    assert_eq!(content(&app, id), "Xedited\n");
    run_the_save_cmd(&mut app, &mut effects);
    assert_eq!(
        mem.read(Path::new(DOC_PATH)).expect("read back"),
        b"Xedited\n"
    );
}

#[test]
fn a_crlf_document_saves_with_every_line_terminator_intact() {
    let (mut app, mem, id) = app_with("one  \r\ntwo\t\r\nthree\r\n");

    let (start, mut effects) = save_interactively(&mut app, id);

    assert_eq!(start, SaveStart::InFlight);
    run_the_save_cmd(&mut app, &mut effects);
    assert_eq!(
        mem.read(Path::new(DOC_PATH)).expect("read back"),
        b"one\r\ntwo\r\nthree\r\n"
    );
}
