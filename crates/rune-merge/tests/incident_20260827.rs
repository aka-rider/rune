//! Regression gate for the 2026-08-26 merge-collapse incident: `ours`
//! deletes a region `theirs` modified, and the old rendered-diff3 parse
//! desynced its two cursors on the conflict's empty ours-section, then
//! classified the whole remaining buffer `Clean("")` — presenting a
//! whole-buffer deletion as a clean merge. Position-accounted hunks make
//! that unrepresentable: ours' lone clean change auto-resolves, and the
//! genuinely double-edited tail is a conflict carrying both sides' bytes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rune_merge::{Hunk, merge_hunks};

fn ours_view(hunks: &[Hunk]) -> Vec<u8> {
    hunks
        .iter()
        .flat_map(|h| match h {
            Hunk::Clean(bytes) => bytes.clone(),
            Hunk::Conflict { ours, .. } => ours.clone(),
        })
        .collect()
}

fn theirs_view(hunks: &[Hunk]) -> Vec<u8> {
    hunks
        .iter()
        .flat_map(|h| match h {
            Hunk::Clean(bytes) => bytes.clone(),
            Hunk::Conflict { theirs, .. } => theirs.clone(),
        })
        .collect()
}

#[test]
fn ours_delete_under_theirs_edit_merges_correctly() {
    let ancestor = b"1\n2\n3\n";
    let ours = b"1x\n2\n";
    let theirs = b"1\n2\n3y\n";

    let hunks = merge_hunks(ancestor, ours, theirs);

    assert_eq!(
        hunks,
        vec![
            Hunk::Clean(b"1x\n2\n".to_vec()),
            Hunk::Conflict {
                ours: b"".to_vec(),
                theirs: b"3y\n".to_vec(),
            },
        ]
    );
    assert_eq!(ours_view(&hunks), ours.to_vec());
    assert_eq!(theirs_view(&hunks), b"1x\n2\n3y\n".to_vec());
}
