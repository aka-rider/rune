//! Unit tests for the edit-batch commit chokepoint, kept in a sibling
//! file so the chokepoint itself stays inside the §1.6 budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
    use crate::commands::edit;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    /// Two independent, non-touching CURSORS — `CursorSet::merge` correctly
    /// leaves positions 0 and 1 separate, since neither's (zero-width)
    /// selection touches the other's — each derive a one-byte Delete-
    /// forward range from their own position: `[0,1)` and `[1,2)`. Those
    /// two DERIVED ranges do touch. Left unmerged, `Buffer::apply_edits`
    /// hands back two `AppliedEdit`s that both land on post-edit `start ==
    /// 0` — the exact illegal state `undo::reapply`'s precondition assert
    /// exists to catch (`crates/rune-fuzz` artifact `no-panic-7f29861c`,
    /// checked in as `repros/no-panic-01.rune`).
    #[test]
    fn merges_two_adjacent_bare_deletes() {
        let infos = vec![
            (
                Edit {
                    start: 1,
                    end: 2,
                    insert: String::new(),
                },
                2,
            ),
            (
                Edit {
                    start: 0,
                    end: 1,
                    insert: String::new(),
                },
                1,
            ),
        ];
        let merged = coalesce_touching_edits(infos);
        assert_eq!(
            merged.len(),
            1,
            "touching ranges must collapse into one edit"
        );
        assert_eq!(
            merged.first(),
            Some(&(
                Edit {
                    start: 0,
                    end: 2,
                    insert: String::new(),
                },
                1,
            )),
            "the lower cursor id survives, matching CursorSet::merge's own rule"
        );
    }

    /// Two cursors sitting inside the same word: a delete-word-right from
    /// each derives OVERLAPPING (not just touching) ranges `[0,5)` and
    /// `[2,7)`. `Buffer::apply_edits` would otherwise reject this batch
    /// outright as `EditsNotSortedOrOverlapping` — a spurious "edit
    /// failed" for an entirely ordinary multi-cursor action.
    #[test]
    fn merges_overlapping_word_deletes() {
        let infos = vec![
            (
                Edit {
                    start: 2,
                    end: 7,
                    insert: String::new(),
                },
                9,
            ),
            (
                Edit {
                    start: 0,
                    end: 5,
                    insert: String::new(),
                },
                3,
            ),
        ];
        let merged = coalesce_touching_edits(infos);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged.first().map(|(e, _)| (e.start, e.end)), Some((0, 7)));
    }

    /// A real gap between two cursors' ranges must survive untouched —
    /// the common case (most multi-cursor edits do not collide at all).
    #[test]
    fn leaves_genuinely_separated_edits_alone() {
        let infos = vec![
            (
                Edit {
                    start: 5,
                    end: 6,
                    insert: String::new(),
                },
                2,
            ),
            (
                Edit {
                    start: 0,
                    end: 1,
                    insert: String::new(),
                },
                1,
            ),
        ];
        let merged = coalesce_touching_edits(infos);
        assert_eq!(merged.len(), 2, "a real gap must not be merged away");
    }

    fn app_with(content: &str) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        let id = app.active;
        app.doc_mut(id)
            .expect("fixture doc must exist")
            .viewport
            .set_size(80, 23);
        app
    }

    /// The regression this fix exists for (root-caused via `make
    /// test-fuzz`, and pinned by the checked-in no-panic replay repro): two cursors, one rune
    /// apart, neither with a selection, both Backspace. Neither cursor's
    /// own selection touches the other's, so `CursorSet::merge` correctly
    /// leaves them as two separate cursors — but Backspace's `bare` range
    /// reaches one rune LEFT of each cursor's position, so the two EDITS
    /// those cursors' commands produce DO touch (`[0,1)` and `[1,2)`).
    ///
    /// Without `coalesce_touching_edits` above, this batch would reach
    /// `Buffer::apply_edits` as two separate touching edits — this test's
    /// own `step.edits.len() == 1` assertion below catches that directly.
    /// Redoing that same two-edit `Step` is what actually trips
    /// `undo::reapply`'s `STRICT_INVARIANTS`-gated assertion in production
    /// (the earlier pure-deletion edit's negative shift collapses the
    /// later edit's post-edit `start` onto it) — not reproduced by THIS
    /// test, since this crate's own test build does not compile `rune-core`
    /// with `cfg(test)` or the `strict-invariants` feature; only the
    /// session fuzzer opts into that, deliberately. The checked-in replay
    /// repro is what proves the reapply panic itself is gone.
    /// Verified by temporarily reverting the `coalesce_touching_edits`
    /// call in `apply_edit_batch_with_cursors` and re-running this test: it
    /// then fails at the `step.edits.len()` assertion below.
    #[test]
    fn two_adjacent_cursors_backspacing_coalesce_into_one_edit_and_survive_redo() {
        let mut app = app_with("ab");
        let id = app.active;
        let doc = app.doc_mut(id).expect("fixture doc must exist");
        doc.cursors = CursorSet::new(1).add(Cursor {
            position: 2,
            anchor: 2,
            desired_col: 0,
            id: 0,
        });
        assert_eq!(
            doc.cursors.len(),
            2,
            "fixture must hold two cursors, one rune apart, for merge() to legitimately leave separate"
        );

        edit::delete_left(&mut app, id);
        assert_eq!(app.doc(id).expect("doc").buffer.content(), "");
        let step = app
            .doc(id)
            .expect("doc")
            .journal
            .undo_peek()
            .expect("one step to undo")
            .0;
        assert_eq!(
            step.edits.len(),
            1,
            "the two cursors' touching Backspace ranges must coalesce into one edit"
        );

        edit::undo(&mut app, id);
        assert_eq!(app.doc(id).expect("doc").buffer.content(), "ab");
        edit::redo(&mut app, id);
        assert_eq!(app.doc(id).expect("doc").buffer.content(), "");
    }

    /// The bug this WP fixes: `delete_selection_or_line` (cut's own
    /// deletion path, with no selection) on an EMPTY buffer derives a
    /// zero-width `Edit { start: 0, end: 0, insert: "" }` from `nav::
    /// line_range_incl_newline` — a legal no-op at the buffer layer, but
    /// committing it used to still bump `version`, push a `Step`, and mark
    /// a clean document dirty.
    #[test]
    fn zero_width_edit_batch_on_an_empty_buffer_does_not_journal_or_dirty() {
        let mut app = app_with("");
        let id = app.active;
        let version_before = app.doc(id).expect("doc").buffer.version();
        let journal_len_before = app.doc(id).expect("doc").journal.len();
        assert!(!app.doc(id).expect("doc").is_dirty());

        edit::delete_selection_or_line(&mut app, id);

        let doc = app.doc(id).expect("doc");
        assert_eq!(
            doc.buffer.version(),
            version_before,
            "a zero-width no-op must not bump the buffer version"
        );
        assert_eq!(
            doc.journal.len(),
            journal_len_before,
            "a zero-width no-op must not journal a step"
        );
        assert!(
            !doc.is_dirty(),
            "a zero-width no-op must not mark a clean document dirty"
        );
    }

    /// Same bug, on the empty LAST line of an otherwise non-empty buffer:
    /// the cursor sits at EOF on a line with nothing after it, so
    /// `nav::line_range_incl_newline` derives `[len, len)` — again
    /// zero-width, since there is no trailing newline past the buffer's
    /// final byte to include.
    #[test]
    fn zero_width_edit_batch_on_an_empty_last_line_does_not_journal_or_dirty() {
        let mut app = app_with("a\n");
        let id = app.active;
        let doc = app.doc_mut(id).expect("doc");
        doc.cursors = CursorSet::new(2);
        let version_before = app.doc(id).expect("doc").buffer.version();
        let journal_len_before = app.doc(id).expect("doc").journal.len();
        assert!(!app.doc(id).expect("doc").is_dirty());

        edit::delete_selection_or_line(&mut app, id);

        let doc = app.doc(id).expect("doc");
        assert_eq!(doc.buffer.content(), "a\n", "the buffer must be untouched");
        assert_eq!(
            doc.buffer.version(),
            version_before,
            "a zero-width no-op must not bump the buffer version"
        );
        assert_eq!(
            doc.journal.len(),
            journal_len_before,
            "a zero-width no-op must not journal a step"
        );
        assert!(
            !doc.is_dirty(),
            "a zero-width no-op must not mark a clean document dirty"
        );
    }

    /// A batch mixing one real edit with one zero-width no-op must still
    /// apply the real edit — the filter drops only the no-op entries, not
    /// the whole batch — and the version must bump exactly once (one
    /// `apply_edits` call, whatever survives the filter).
    #[test]
    fn mixed_batch_keeps_its_real_edits_and_drops_the_no_ops() {
        let mut app = app_with("ab");
        let id = app.active;
        let version_before = app.doc(id).expect("doc").buffer.version();
        let cursors_before = app.doc(id).expect("doc").cursors.clone();

        let infos = vec![
            (
                Edit {
                    start: 0,
                    end: 1,
                    insert: String::new(),
                },
                0,
            ),
            (
                Edit {
                    start: 2,
                    end: 2,
                    insert: String::new(),
                },
                1,
            ),
        ];
        commit_edit_batch(&mut app, id, infos, cursors_before);

        let doc = app.doc(id).expect("doc");
        assert_eq!(
            doc.buffer.content(),
            "b",
            "the real edit must still apply"
        );
        assert_eq!(
            doc.buffer.version(),
            version_before + 1,
            "the surviving batch must bump the version exactly once"
        );
    }

    /// Pins `coalesce_touching_edits`'s cursor-survivor rule (review
    /// finding this test closes: nothing previously asserted the post-edit
    /// cursor COUNT, only the merged edit's range): two touching per-cursor
    /// deletes must collapse the cursor set down to the lower-id survivor,
    /// mirroring `CursorSet::merge`. This pins current behaviour; it does
    /// not change `coalesce_touching_edits` itself.
    #[test]
    fn touching_per_cursor_edits_collapse_to_one_surviving_cursor() {
        let mut app = app_with("ab");
        let id = app.active;
        let doc = app.doc_mut(id).expect("doc");
        doc.cursors = CursorSet::new(1).add(Cursor {
            position: 2,
            anchor: 2,
            desired_col: 0,
            id: 0,
        });
        assert_eq!(doc.cursors.len(), 2, "fixture must hold two cursors");

        edit::delete_left(&mut app, id);

        let doc = app.doc(id).expect("doc");
        assert_eq!(doc.buffer.content(), "");
        let cursors = doc.cursors.all();
        assert_eq!(
            cursors.len(),
            1,
            "the touching pair must collapse to a single surviving cursor"
        );
        assert_eq!(
            cursors[0].id, 1,
            "the lower cursor id must be the survivor, matching CursorSet::merge's own rule"
        );
    }
