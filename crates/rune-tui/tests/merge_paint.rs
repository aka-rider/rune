//! Plan WP5 "Done when": a render test asserting ours-span cells carry
//! `merge_ours_bg`, theirs-span `merge_theirs_bg`, marker lines
//! `merge_marker_bg`; a resolved block's region carries none; the current
//! block's cue differs from other unresolved blocks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod highlight_common;

use highlight_common::app_for;
use ratatui::buffer::Buffer as RtBuffer;
use rune_merge::Hunk;
use rune_tui::app::App;
use rune_tui::merge::frame::build_marker_buffer;
use rune_tui::merge::state::MergeState;
use rune_tui::testgrid;

const W: u16 = 40;
const H: u16 = 20;

fn draw(app: &App) -> RtBuffer {
    testgrid::draw(app, W, H)
}

fn sized_app_with_merge(buffer: &str, mut install: impl FnMut(&mut App)) -> App {
    let mut app = app_for(buffer, "/x/notes.md");
    app.doc_mut(app.active)
        .expect("doc")
        .viewport
        .set_size(W, H);
    install(&mut app);
    app.sync_view();
    app
}

fn row_text(buf: &RtBuffer, y: u16) -> String {
    (0..W)
        .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
        .collect()
}

fn row_containing(buf: &RtBuffer, needle: &str) -> u16 {
    (0..H)
        .find(|&y| row_text(buf, y).contains(needle))
        .unwrap_or_else(|| panic!("no rendered row contains {needle:?}"))
}

fn cell_bg(buf: &RtBuffer, x: u16, y: u16) -> Option<ratatui::style::Color> {
    buf.cell((x, y)).and_then(|c| c.style().bg)
}

/// Two unresolved blocks (so a "current vs. other" comparison is possible)
/// separated by a clean line — built through the real `build_marker_buffer`
/// so the byte offsets `merge::paint` consumes are the real deterministic
/// framing, not a hand-typed fixture that could drift from `frame_block`.
fn two_block_fixture() -> (
    String,
    Vec<rune_tui::merge::state::Block>,
    Vec<rune_tui::merge::state::Conflict>,
) {
    let hunks = vec![
        Hunk::Conflict {
            ours: b"mine one".to_vec(),
            theirs: b"yours one".to_vec(),
        },
        Hunk::Clean(b"between\n".to_vec()),
        Hunk::Conflict {
            ours: b"mine two".to_vec(),
            theirs: b"yours two".to_vec(),
        },
    ];
    build_marker_buffer(&hunks).expect("valid utf8 fixture")
}

#[test]
fn ours_theirs_and_marker_spans_paint_their_own_background_on_screen() {
    let (buffer, blocks, conflicts) = two_block_fixture();
    let app = sized_app_with_merge(&buffer, |app| {
        app.merge = MergeState::Active {
            doc: app.active,
            conflicts: conflicts.clone(),
            blocks: blocks.clone(),
            cur: 0,
            saved_display_name: None,
            theirs_obs: rune_db::ObsId::new(1).expect("nonzero"),
        };
    });
    let buf = draw(&app);

    let ours_row = row_containing(&buf, "mine one");
    let theirs_row = row_containing(&buf, "yours one");
    let marker_row = row_containing(&buf, "<<<<<<<");
    let sep_row = row_containing(&buf, "=======");

    let ours_col = row_text(&buf, ours_row).find("mine one").unwrap() as u16;
    let theirs_col = row_text(&buf, theirs_row).find("yours one").unwrap() as u16;
    let marker_col = row_text(&buf, marker_row).find('<').unwrap() as u16;
    let sep_col = row_text(&buf, sep_row).find('=').unwrap() as u16;

    assert_eq!(
        cell_bg(&buf, ours_col, ours_row),
        Some(app.theme.chrome.merge_ours_bg.bg.unwrap()),
        "ours span must carry merge_ours_bg"
    );
    assert_eq!(
        cell_bg(&buf, theirs_col, theirs_row),
        Some(app.theme.chrome.merge_theirs_bg.bg.unwrap()),
        "theirs span must carry merge_theirs_bg"
    );
    assert_eq!(
        cell_bg(&buf, marker_col, marker_row),
        Some(app.theme.chrome.merge_marker_bg.bg.unwrap()),
        "the ours marker line must carry merge_marker_bg"
    );
    assert_eq!(
        cell_bg(&buf, sep_col, sep_row),
        Some(app.theme.chrome.merge_marker_bg.bg.unwrap()),
        "the separator marker line must carry merge_marker_bg"
    );
}

#[test]
fn a_resolved_blocks_region_carries_no_merge_background() {
    let (buffer, mut blocks, conflicts) = two_block_fixture();
    blocks[0].resolved = true;
    let app = sized_app_with_merge(&buffer, |app| {
        app.merge = MergeState::Active {
            doc: app.active,
            conflicts: conflicts.clone(),
            blocks: blocks.clone(),
            cur: 1,
            saved_display_name: None,
            theirs_obs: rune_db::ObsId::new(1).expect("nonzero"),
        };
    });
    let buf = draw(&app);

    let ours_row = row_containing(&buf, "mine one");
    let ours_col = row_text(&buf, ours_row).find("mine one").unwrap() as u16;
    assert_ne!(
        cell_bg(&buf, ours_col, ours_row),
        Some(app.theme.chrome.merge_ours_bg.bg.unwrap()),
        "a resolved block's ours span must carry no merge background"
    );
    let marker_row = row_containing(&buf, "<<<<<<<");
    let marker_col = row_text(&buf, marker_row).find('<').unwrap() as u16;
    assert_ne!(
        cell_bg(&buf, marker_col, marker_row),
        Some(app.theme.chrome.merge_marker_bg.bg.unwrap()),
        "a resolved block's marker line must carry no merge background"
    );
}

#[test]
fn the_current_blocks_marker_carries_a_distinct_cue() {
    let (buffer, blocks, conflicts) = two_block_fixture();
    let app = sized_app_with_merge(&buffer, |app| {
        app.merge = MergeState::Active {
            doc: app.active,
            conflicts: conflicts.clone(),
            blocks: blocks.clone(),
            cur: 0,
            saved_display_name: None,
            theirs_obs: rune_db::ObsId::new(1).expect("nonzero"),
        };
    });
    let buf = draw(&app);

    let markers: Vec<u16> = (0..H)
        .filter(|&y| row_text(&buf, y).contains("<<<<<<<"))
        .collect();
    assert_eq!(
        markers.len(),
        2,
        "fixture has two marker lines, one per block"
    );
    let col0 = row_text(&buf, markers[0]).find('<').unwrap() as u16;
    let col1 = row_text(&buf, markers[1]).find('<').unwrap() as u16;
    let current_marker = buf
        .cell((col0, markers[0]))
        .expect("current block's marker row");
    let other_marker = buf
        .cell((col1, markers[1]))
        .expect("other block's marker row");

    assert_eq!(
        current_marker.style().bg,
        other_marker.style().bg,
        "both are still the same marker background colour"
    );
    assert_ne!(
        current_marker.style().add_modifier,
        other_marker.style().add_modifier,
        "the current block's marker must carry a visibly distinct cue"
    );
}
