//! Tests for `undo`, split out to keep the owning module under the
//! 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

#[test]
fn undo_then_redo_round_trips_content() {
    let buf = Buffer::new("hello world");
    let (edited, applied) = buf
        .apply_edits(&SortedEdits::single(Edit {
            start: 5,
            end: 11,
            insert: " rust".to_string(),
        }))
        .expect("edit should apply");
    assert_eq!(edited.content(), "hello rust");

    let restored = apply_inverse(&edited, &applied).expect("inverse should apply");
    assert_eq!(restored.content(), "hello world");

    let redone = reapply(&restored, &applied).expect("reapply should apply");
    assert_eq!(redone.content(), "hello rust");
}

/// Documents the exact illegal-edit-set mechanism `apply_edits`'
/// `DuplicateEditStart` rejection exists to catch, at the
/// buffer-primitive level: two INDEPENDENT zero-width byte positions (0
/// and 1 — not touching as cursor points, so `cursor::CursorSet::merge`
/// correctly leaves them as two separate cursors) each deleting one
/// byte forward derive ADJACENT, touching ranges `[0,1)` and `[1,2)`,
/// both landing on post-edit `start == 0`: the identical illegal state
/// `reapply` cannot safely order. `Buffer::apply_edits` refuses the
/// batch outright now (see `apply_edits_rejects_a_batch_whose_edits_
/// collide_on_post_edit_start` in `buffer.rs`) rather than handing the
/// colliding `AppliedEdit`s to a caller — this test pins that the
/// rejection fires for exactly the batch shape that used to slip
/// through: a touching, non-overlapping pair that
/// `coalesce_touching_edits` (in `rune-tui`, which this crate cannot see
/// or depend on) would have merged before it ever reached here.
#[test]
fn adjacent_bare_deletes_collide_on_the_same_post_edit_start() {
    let buf = Buffer::new("ab");
    let sorted = SortedEdits::sort(&[
        Edit {
            start: 1,
            end: 2,
            insert: String::new(),
        },
        Edit {
            start: 0,
            end: 1,
            insert: String::new(),
        },
    ]);
    let err = buf.apply_edits(&sorted);
    assert_eq!(
        err,
        Err(BufferError::DuplicateEditStart { start: 0 }),
        "two adjacent one-byte deletes would collapse to the identical \
         post-edit start — apply_edits must refuse the batch outright"
    );
}

/// The other half of the pin above, at `reapply`'s own boundary: a
/// hand-built `AppliedEdit` batch with colliding starts — the shape a
/// persisted journal row written before `apply_edits` enforced this
/// invariant could still carry — is refused with the same
/// `BufferError`, not replayed in whatever order the tied sort
/// produces.
#[test]
fn reapply_refuses_a_batch_with_colliding_starts() {
    let buf = Buffer::new("ab");
    let applied = vec![
        AppliedEdit {
            start: 0,
            end: 0,
            deleted: "b".to_string(),
            insert: String::new(),
        },
        AppliedEdit {
            start: 0,
            end: 0,
            deleted: "a".to_string(),
            insert: String::new(),
        },
    ];
    let err = reapply(&buf, &applied);
    assert_eq!(err, Err(BufferError::DuplicateEditStart { start: 0 }));
}

/// Pins the `UNDO-TOTAL` multi-cursor regression this fix closes,
/// reduced to `Buffer`/`AppliedEdit` primitives: two cursors sharing
/// ONE line (the `clone-line` shape — `rune-tui`'s
/// `edit_lines::per_line_edits(dedupe=false)` lets both build an edit
/// for the same line) each clone that line, so BOTH forward edits are
/// pure inserts at the IDENTICAL pre-edit `start`. `Buffer::apply_edits`
/// accepts that batch — the two `AppliedEdit`s land on DISTINCT
/// post-edit starts (one clone's insert shifts the other's), so this
/// is a perfectly legal, undoable step, never `DuplicateEditStart`.
///
/// Undoing it is a different story: `inverse_edits` turns each pure
/// INSERT into a pure DELETE at the SAME post-edit start/end the
/// forward apply computed — and those two deletes are exactly
/// touching (one's end is the other's start), which — left
/// unmerged — would themselves collide once shifted, tripping
/// `DuplicateEditStart` on the very undo the forward edit legitimately
/// earned. `coalesce_touching_deletes` (called from `inverse_edits`)
/// exists to merge exactly this touching-pure-delete pair before the
/// buffer ever sees it, so `apply_inverse` here must succeed and
/// restore the pre-clone content exactly.
#[test]
fn apply_inverse_undoes_a_two_cursor_same_line_clone() {
    let buf = Buffer::new("\nhello world");
    let line = "hello world";
    // Both cursors are on line 1 (`per_line_edits(dedupe=false)`), so
    // both clone edits share the SAME pre-edit `start`/`end` — the
    // line's own start — exactly like `edit_lines::clone_line_up`'s
    // real construction.
    let clone_edit = || Edit {
        start: 1,
        end: 1,
        insert: format!("{line}\n"),
    };
    let (cloned, applied) = buf
        .apply_edits(&SortedEdits::sort(&[clone_edit(), clone_edit()]))
        .expect("two same-line pure-insert clones must not collide going forward");
    assert_eq!(cloned.content(), "\nhello world\nhello world\nhello world");
    assert_eq!(applied.len(), 2, "one AppliedEdit per cursor's clone");
    assert_ne!(
        applied[0].start, applied[1].start,
        "the two clones must land on distinct post-edit starts, or the \
         forward apply itself should have refused the batch"
    );

    let restored = apply_inverse(&cloned, &applied)
        .expect("undo must succeed: the forward step it inverts was itself legal");
    assert_eq!(
        restored.content(),
        buf.content(),
        "undo must restore the exact pre-clone content"
    );
}

#[test]
fn journal_peek_does_not_move_position_until_committed() {
    let mut journal = Journal::new();
    journal.push(Step::default());
    assert_eq!(journal.pos(), 1);

    let (_, new_pos) = journal.undo_peek().expect("one step to undo");
    assert_eq!(journal.pos(), 1, "peek must not move pos");
    journal.commit(new_pos);
    assert_eq!(journal.pos(), 0);

    assert!(journal.undo_peek().is_none());
    let (_, redo_pos) = journal.redo_peek().expect("one step to redo");
    assert_eq!(journal.pos(), 0, "peek must not move pos");
    journal.commit(redo_pos);
    assert_eq!(journal.pos(), 1);
}

#[test]
fn push_truncates_redo_tail() {
    let mut journal = Journal::new();
    journal.push(Step::default());
    journal.push(Step::default());
    let (_, pos) = journal.undo_peek().expect("one step to undo");
    journal.commit(pos);
    journal.push(Step::default());
    assert_eq!(journal.len(), 2, "the discarded redo step must be gone");
    assert_eq!(journal.pos(), 2);
}

/// A pure INSERT (non-empty `insert`) touching a pure DELETE must never
/// merge, even though they sit adjacent in the sorted batch: merging
/// would silently discard the insert's text and turn it into more
/// deletion. `&&` requires BOTH sides of the pair to be pure deletes;
/// weakening it to `||` would merge here because the delete edit alone
/// satisfies one side.
#[test]
fn coalesce_touching_deletes_never_merges_a_delete_with_a_pure_insert() {
    let insert_edit = Edit {
        start: 0,
        end: 0,
        insert: "X".to_string(),
    };
    let delete_edit = Edit {
        start: 0,
        end: 3,
        insert: String::new(),
    };
    let merged = coalesce_touching_deletes(
        vec![(insert_edit.clone(), 'a'), (delete_edit.clone(), 'b')],
        |a, _b| a,
    );
    assert_eq!(merged, vec![(insert_edit, 'a'), (delete_edit, 'b')]);
}

/// The executed regression this fix closes: a forward batch pairing a
/// pure insert with a replace, touching at the insert's zero-width point,
/// is legally accepted going forward (the two `AppliedEdit`s land on
/// distinct post-edit starts) — but its inverse used to pair a pure
/// delete with a non-pure-delete edit that touches it, a shape the old
/// `both_pure_deletes` condition refused to merge, so `apply_inverse`
/// wrongly returned `DuplicateEditStart` for an undo the forward step
/// itself had legitimately earned.
#[test]
fn apply_inverse_undoes_a_batch_pairing_a_pure_delete_inverse_with_a_replace() {
    let buf = Buffer::new("x");
    let (edited, applied) = buf
        .apply_edits(&SortedEdits::sort(&[
            Edit {
                start: 0,
                end: 0,
                insert: "AB".to_string(),
            },
            Edit {
                start: 0,
                end: 1,
                insert: "YYY".to_string(),
            },
        ]))
        .expect("touching, non-overlapping edits with distinct post-edit starts must apply");
    assert_eq!(applied.len(), 2);
    assert_ne!(
        applied[0].start, applied[1].start,
        "the forward batch itself must not collide, or apply_edits should have refused it"
    );

    let restored = apply_inverse(&edited, &applied)
        .expect("undo must succeed: the forward step it inverts was itself legal");
    assert_eq!(restored.content(), "x");
}

/// Property-style regression for the same fix: over a bounded set of
/// deterministically generated (fixed-seed, no wall-clock) small edit
/// batches, whenever `Buffer::apply_edits` accepts a batch,
/// `apply_inverse` must restore the exact pre-edit content.
#[test]
fn any_batch_apply_edits_accepts_undoes_back_to_the_original_content() {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next_u64 = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet = *b"ab\n";

    for _ in 0..500 {
        let content_len = (next_u64() % 12) as usize;
        let content: String = (0..content_len)
            .map(|_| alphabet[(next_u64() % alphabet.len() as u64) as usize] as char)
            .collect();
        let buf = Buffer::new(content.clone());
        let len = buf.len();

        let batch_len = 1 + (next_u64() % 3) as usize;
        let edits: Vec<Edit> = (0..batch_len)
            .map(|_| {
                let start = (next_u64() % (len as u64 + 1)) as usize;
                let extra = (next_u64() % 4) as usize;
                let end = (start + extra).min(len);
                let insert_len = (next_u64() % 3) as usize;
                let insert: String = (0..insert_len)
                    .map(|_| alphabet[(next_u64() % alphabet.len() as u64) as usize] as char)
                    .collect();
                Edit { start, end, insert }
            })
            .collect();

        if let Ok((edited, applied)) = buf.apply_edits(&SortedEdits::sort(&edits)) {
            let restored = apply_inverse(&edited, &applied)
                .expect("any batch apply_edits accepted must have an applicable inverse");
            assert_eq!(
                restored.content(),
                content,
                "undo must restore the exact pre-edit content for batch {edits:?}"
            );
        }
    }
}

#[test]
fn journal_commit_target_is_the_step_index_it_carries() {
    let mut journal = Journal::new();
    journal.push(Step::default());
    journal.push(Step::default());
    journal.push(Step::default());
    let (_, commit) = journal.undo_peek().expect("three steps to undo from");
    assert_eq!(commit.target(), 2);
}

#[test]
fn steps_returns_exactly_what_was_pushed() {
    let mut journal = Journal::new();
    let step_a = Step {
        kind: EditKind::Insert,
        ..Step::default()
    };
    let step_b = Step {
        kind: EditKind::Cut,
        ..Step::default()
    };
    journal.push(step_a.clone());
    journal.push(step_b.clone());
    assert_eq!(journal.steps(), &[step_a, step_b]);
}

#[test]
fn journal_is_empty_reflects_whether_any_step_was_pushed() {
    let mut journal = Journal::new();
    assert!(journal.is_empty());
    journal.push(Step::default());
    assert!(!journal.is_empty());
}
