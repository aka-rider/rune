#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use rune_db::SyncKind;
use rune_fuzz::Session;
use rune_tui::document::DocumentId;
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;
use rune_tui::testgrid;

use merge_common::{bare, ch, ctrl, external_write, reprobe, take_theirs, untitled_draft};

const W: u16 = 83;
const H: u16 = 24;

const ANCESTOR: &str = "one\ntwo\nthree\nfour\nfive\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\n";

fn enter_two_conflict_merge() -> (Session, DocumentId) {
    let mut session = Session::open("/doc.md", ANCESTOR);
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('X')).is_none());
    for _ in 0..4 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::End)).is_none());
    assert!(session.key(ch('Z')).is_none());
    assert!(session.deliver_db_all().is_none());

    external_write(session.app().vfs.as_ref(), THEIRS);
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());
    assert!(matches!(session.app().merge, MergeState::Active { .. }));
    assert!(session.resize(W, H).is_none());
    (session, doc_id)
}

fn grid_row_with(grid: &[String], needle: &str) -> usize {
    grid.iter()
        .position(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("no rendered row contains {needle:?}"))
}

fn col_of(row: &str, needle: &str) -> u16 {
    let byte_idx = row.find(needle).expect("needle present in row");
    row[..byte_idx].chars().count() as u16
}

fn col_of_right_pane(row: &str, needle: &str) -> u16 {
    let byte_idx = row.rfind(needle).expect("needle present in row");
    row[..byte_idx].chars().count() as u16
}

#[test]
fn conflict_regions_carry_theirs_bg_left_and_ours_bg_right() {
    let (session, _doc) = enter_two_conflict_merge();
    let app = session.app();
    let grid = testgrid::grid(app, W, H);
    let buf = testgrid::draw(app, W, H);

    let row = grid_row_with(&grid, "one disk");
    assert!(
        grid[row].contains("Xone"),
        "the aligned ours line renders on the same visual row: {:?}",
        grid[row]
    );

    let left_col = col_of(&grid[row], "one");
    let left_bg = buf.cell((left_col, row as u16)).and_then(|c| c.style().bg);
    assert!(
        left_bg == app.theme.chrome.merge_theirs_bg.bg
            || left_bg == app.theme.chrome.diff_word_theirs.bg,
        "the left (disk) side of a conflict carries a theirs-side background, got {left_bg:?}"
    );

    let right_col = col_of_right_pane(&grid[row], "one");
    let right_bg = buf.cell((right_col, row as u16)).and_then(|c| c.style().bg);
    assert!(
        right_bg == app.theme.chrome.merge_ours_bg.bg
            || right_bg == app.theme.chrome.diff_word_ours.bg,
        "the right (ours) side of a conflict carries an ours-side background, got {right_bg:?}"
    );
}

#[test]
fn a_taken_conflicts_region_loses_its_backgrounds() {
    let (mut session, _doc) = enter_two_conflict_merge();

    assert!(session.key(take_theirs()).is_none());
    assert!(session.key(merge_common::prev_hunk()).is_none());

    let app = session.app();
    let grid = testgrid::grid(app, W, H);
    let buf = testgrid::draw(app, W, H);

    let row = grid_row_with(&grid, "one disk");
    let right_col = col_of_right_pane(&grid[row], "one disk");
    let right_bg = buf.cell((right_col, row as u16)).and_then(|c| c.style().bg);
    assert_ne!(
        right_bg, app.theme.chrome.merge_ours_bg.bg,
        "a taken conflict's region is Same in the live diff and must carry no ours background"
    );
    assert_ne!(
        right_bg, app.theme.chrome.diff_word_ours.bg,
        "a taken conflict's region must carry no intraline emphasis either"
    );

    let unresolved_row = grid_row_with(&grid, "five disk");
    let left_col = col_of(&grid[unresolved_row], "five");
    let left_bg = buf
        .cell((left_col, unresolved_row as u16))
        .and_then(|c| c.style().bg);
    assert!(
        left_bg == app.theme.chrome.merge_theirs_bg.bg
            || left_bg == app.theme.chrome.diff_word_theirs.bg,
        "the still-unresolved conflict keeps its theirs-side background, got {left_bg:?}"
    );
}
