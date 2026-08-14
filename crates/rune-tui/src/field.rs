//! The editing core a single-line field reuses — cursor motion, selection,
//! in-place editing, and an in-memory undo/redo journal, all keyed by BYTE
//! offsets. Built directly on `rune_core::{Buffer, Cursor,
//! undo::Journal}` so every mutation gets range clamping and UTF-8
//! validation for free, and the `AppliedEdit`s that `undo::apply_inverse`/
//! `reapply` invert.
//!
//! This `Journal` is in-memory only and is never replicated to the
//! recovery store — that governs exactly what the title field needs: a
//! rename is one atomic bind, and the field's own undo history plays no
//! part in it.
//!
//! Every mutating method takes a `window: Range<usize>` — the caller's
//! currently-editable sub-range of the field's text. `TextField` never
//! stores a window itself; the caller recomputes it fresh on every call,
//! so nothing here can drift out of sync with a caller-side gate.

use std::ops::Range;

use rune_core::buffer::{Buffer, Edit};
use rune_core::cursor::{Cursor, CursorSet};
use rune_core::undo::{self, EditKind, Journal, Step};

use crate::commands::nav;
use crate::keymap::{Command, Extend, KeyOutcome, Motion};

/// A single-line, undoable text editing core. Holds the full text in one
/// `Buffer` — never two separately-tracked strings — so a caller that
/// wants to fence off part of the text (the title's extension gate) can
/// derive the boundary from the live content instead of losing the
/// ability to edit across it.
pub struct TextField {
    buffer: Buffer,
    cursor: Cursor,
    journal: Journal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeleteDirection {
    Backward,
    Forward,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeleteUnit {
    Rune,
    Word,
}

impl TextField {
    /// A field seeded with `text`, cursor at the end, no selection, empty
    /// undo history.
    pub fn new(text: &str) -> TextField {
        let buffer = Buffer::new(text);
        let cursor = CursorSet::new(buffer.len()).primary();
        TextField {
            buffer,
            cursor,
            journal: Journal::new(),
        }
    }

    pub fn text(&self) -> &str {
        self.buffer.content()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Full reset: replaces the text, moves the cursor to the end, and
    /// discards the undo history — a stale `Undo` after this must be a
    /// no-op, not a resurrection of the previous text.
    pub fn set_text(&mut self, text: &str) {
        let buffer = Buffer::new(text);
        let len = buffer.len();
        let id = self.cursor.id;
        self.buffer = buffer;
        self.cursor = Cursor {
            position: len,
            anchor: len,
            desired_col: 0,
            id,
        };
        self.journal = Journal::new();
    }

    /// Places the cursor/selection directly, clamped into `0..=len` and
    /// snapped to the nearest `char` boundary in both directions.
    pub fn set_cursor(&mut self, position: usize, anchor: usize) {
        let whole = 0..self.buffer.len();
        let id = self.cursor.id;
        self.cursor = Cursor {
            position: clamp_boundary(self.buffer.content(), position, &whole),
            anchor: clamp_boundary(self.buffer.content(), anchor, &whole),
            desired_col: 0,
            id,
        };
    }

    /// The selected text, or `""` when `cursor` carries no selection.
    pub fn selected_text(&self) -> &str {
        if !self.cursor.has_selection() {
            return "";
        }
        let (start, end) = self.cursor.selection_range();
        self.buffer.slice(start, end).unwrap_or("")
    }

    /// Runs one resolved editor `Command` against `window`, the caller's
    /// live editable sub-range. Commands this field doesn't act on
    /// (vertical motion, paging, clipboard, save, quit, ...) are the
    /// caller's own responsibility and are ignored here.
    pub fn apply(&mut self, cmd: Command, window: Range<usize>) -> KeyOutcome {
        match cmd {
            Command::Motion(Motion::CharLeft, extend) => {
                self.step_left(&window, extend == Extend::Yes, nav::prev_rune_offset)
            }
            Command::Motion(Motion::CharRight, extend) => {
                self.step_right(&window, extend == Extend::Yes, nav::next_rune_offset)
            }
            Command::Motion(Motion::WordLeft, extend) => {
                self.step_left(&window, extend == Extend::Yes, nav::word_left_offset)
            }
            Command::Motion(Motion::WordRight, extend) => {
                self.step_right(&window, extend == Extend::Yes, nav::word_right_offset)
            }
            Command::Motion(Motion::LineStart, extend) => {
                self.move_to(window.start, extend == Extend::Yes);
                KeyOutcome::Consumed
            }
            Command::Motion(Motion::LineEnd, extend) => {
                self.move_to(window.end, extend == Extend::Yes);
                KeyOutcome::Consumed
            }
            Command::SelectAll => {
                self.cursor.anchor = window.start;
                self.cursor.position = window.end;
                self.cursor.desired_col = 0;
                KeyOutcome::Consumed
            }
            Command::DeleteLeft => {
                self.delete_bare(&window, DeleteDirection::Backward, DeleteUnit::Rune)
            }
            Command::DeleteRight => {
                self.delete_bare(&window, DeleteDirection::Forward, DeleteUnit::Rune)
            }
            Command::DeleteWordLeft => {
                self.delete_bare(&window, DeleteDirection::Backward, DeleteUnit::Word)
            }
            Command::DeleteWordRight => {
                self.delete_bare(&window, DeleteDirection::Forward, DeleteUnit::Word)
            }
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            _ => KeyOutcome::Ignored,
        }
    }

    /// Replaces the current selection (or inserts at the cursor when
    /// there is none) with `text`, clamped into `window`.
    pub fn insert(&mut self, text: &str, window: Range<usize>) -> KeyOutcome {
        let (start, end) = if self.cursor.has_selection() {
            self.cursor.selection_range()
        } else {
            (self.cursor.position, self.cursor.position)
        };
        self.edit(start, end, text, &window)
    }

    /// Deletes exactly `range` — the caller (a selection, or a computed
    /// window-relative range) has already decided what should go, so this
    /// clamps only into the live buffer, not into any further window.
    pub fn delete_range(&mut self, range: Range<usize>) -> KeyOutcome {
        let whole = 0..self.buffer.len();
        self.edit(range.start, range.end, "", &whole)
    }

    fn step_left(
        &mut self,
        window: &Range<usize>,
        select: bool,
        step: impl Fn(&Buffer, usize) -> usize,
    ) -> KeyOutcome {
        let offset = if !select && self.cursor.has_selection() {
            self.cursor.selection_start()
        } else {
            step(&self.buffer, self.cursor.position)
        };
        self.move_to(
            clamp_boundary(self.buffer.content(), offset, window),
            select,
        );
        KeyOutcome::Consumed
    }

    fn step_right(
        &mut self,
        window: &Range<usize>,
        select: bool,
        step: impl Fn(&Buffer, usize) -> usize,
    ) -> KeyOutcome {
        let offset = if !select && self.cursor.has_selection() {
            self.cursor.selection_end()
        } else {
            step(&self.buffer, self.cursor.position)
        };
        self.move_to(
            clamp_boundary(self.buffer.content(), offset, window),
            select,
        );
        KeyOutcome::Consumed
    }

    /// The anchor rule shared by every horizontal/line-start-end motion:
    /// move the head to `offset`, and drag the tail along only when
    /// `select` is false.
    fn move_to(&mut self, offset: usize, select: bool) {
        self.cursor.position = offset;
        if !select {
            self.cursor.anchor = offset;
        }
        self.cursor.desired_col = 0;
    }

    /// The range a bare (non-selection) delete targets: one rune or one
    /// word, from the cursor toward `window`'s near edge. A live
    /// selection always wins over the bare range.
    fn delete_bare(
        &mut self,
        window: &Range<usize>,
        direction: DeleteDirection,
        unit: DeleteUnit,
    ) -> KeyOutcome {
        let (start, end) = if self.cursor.has_selection() {
            self.cursor.selection_range()
        } else if direction == DeleteDirection::Backward {
            let target = match unit {
                DeleteUnit::Word => nav::word_left_offset(&self.buffer, self.cursor.position),
                DeleteUnit::Rune => nav::prev_rune_offset(&self.buffer, self.cursor.position),
            };
            (target, self.cursor.position)
        } else {
            let target = match unit {
                DeleteUnit::Word => nav::word_right_offset(&self.buffer, self.cursor.position),
                DeleteUnit::Rune => nav::next_rune_offset(&self.buffer, self.cursor.position),
            };
            (self.cursor.position, target)
        };
        self.edit(start, end, "", window)
    }

    /// The one place a mutation reaches `Buffer::apply_edits` — every
    /// other method above funnels through this. An edit whose range
    /// collapses entirely outside `window` clamps both ends to the same
    /// point and lands as a no-op, rather than needing a special-cased
    /// guard at the call site.
    fn edit(
        &mut self,
        start: usize,
        end: usize,
        insert: &str,
        window: &Range<usize>,
    ) -> KeyOutcome {
        let start = clamp_boundary(self.buffer.content(), start, window);
        let end = clamp_boundary(self.buffer.content(), end, window);
        if start == end && insert.is_empty() {
            return KeyOutcome::Ignored;
        }
        let before = self.cursor;
        let edits = [Edit {
            start,
            end,
            insert: insert.to_string(),
        }];
        let Ok((new_buf, applied)) = self.buffer.apply_edits(&edits) else {
            return KeyOutcome::Ignored; // refuse an out-of-range edit rather than corrupt the buffer
        };
        let landed = applied.last().map_or(start, |a| a.end);
        self.buffer = new_buf;
        self.cursor = Cursor {
            position: landed,
            anchor: landed,
            desired_col: 0,
            id: before.id,
        };
        self.journal.push(Step {
            edits: applied,
            cursors_before: vec![before],
            cursors_after: vec![self.cursor],
            kind: EditKind::Other,
        });
        KeyOutcome::Consumed
    }

    /// Peek-then-commit undo, mirroring the document editor's own
    /// undo/redo commands: the buffer edit must succeed before the
    /// journal position moves, so a failed inverse never runs the journal
    /// ahead of the buffer.
    fn undo(&mut self) -> KeyOutcome {
        let Some((step, new_pos)) = self.journal.undo_peek() else {
            return KeyOutcome::Ignored;
        };
        let edits = step.edits.clone();
        let cursors_before = step.cursors_before.clone();
        let Ok(new_buf) = undo::apply_inverse(&self.buffer, &edits) else {
            return KeyOutcome::Ignored;
        };
        self.buffer = new_buf;
        self.restore_cursor(cursors_before.first().copied());
        self.journal.commit(new_pos);
        KeyOutcome::Consumed
    }

    /// Mirrors `undo` for redo.
    fn redo(&mut self) -> KeyOutcome {
        let Some((step, new_pos)) = self.journal.redo_peek() else {
            return KeyOutcome::Ignored;
        };
        let edits = step.edits.clone();
        let cursors_after = step.cursors_after.clone();
        let Ok(new_buf) = undo::reapply(&self.buffer, &edits) else {
            return KeyOutcome::Ignored;
        };
        self.buffer = new_buf;
        self.restore_cursor(cursors_after.first().copied());
        self.journal.commit(new_pos);
        KeyOutcome::Consumed
    }

    /// Restores a recorded cursor, clamped into the (possibly now
    /// shorter) buffer, with the field's own `id` preserved — never the
    /// zero-`id` "not a real cursor" sentinel, even when `recorded` is
    /// `None`.
    fn restore_cursor(&mut self, recorded: Option<Cursor>) {
        let len = self.buffer.len();
        let id = self.cursor.id;
        let restored = recorded.unwrap_or(self.cursor);
        self.cursor = Cursor {
            position: restored.position.min(len),
            anchor: restored.anchor.min(len),
            desired_col: 0,
            id,
        };
    }
}

/// Clamps `offset` into `window` and walks it down to the nearest `char`
/// boundary — so a start/end derived from a caller-chosen window can
/// never reach `Buffer::apply_edits` mid-codepoint.
fn clamp_boundary(content: &str, offset: usize, window: &Range<usize>) -> usize {
    let clamped = offset.max(window.start).min(window.end);
    content.floor_char_boundary(clamped)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// `nav::word_left_offset`/`word_right_offset` classify a single `.`
    /// between two letters as word-forming (Unicode's `MidNumLet` word-
    /// break rule, the same one that keeps `example.com` one word) — so
    /// `a.b` is one word and `c` is the other, not three tokens split on
    /// the dot. Word motion over "a.b c" therefore has exactly two
    /// forward stops (end of `a.b`, end of `c`) and two backward stops
    /// (start of `c`, start of `a.b`); a further motion at either end is
    /// idempotent.
    #[test]
    fn word_left_and_right_step_over_dot_and_space() {
        let mut field = TextField::new("a.b c");
        field.set_cursor(0, 0);
        let window = 0..field.len();
        for expected in [3, 5, 5] {
            let outcome = field.apply(
                Command::Motion(Motion::WordRight, Extend::No),
                window.clone(),
            );
            assert_eq!(outcome, KeyOutcome::Consumed);
            assert_eq!(field.cursor().position, expected);
        }
        for expected in [4, 0, 0] {
            let outcome = field.apply(
                Command::Motion(Motion::WordLeft, Extend::No),
                window.clone(),
            );
            assert_eq!(outcome, KeyOutcome::Consumed);
            assert_eq!(field.cursor().position, expected);
        }
    }

    #[test]
    fn shift_selection_extends_then_plain_arrow_collapses_to_the_edge() {
        let mut field = TextField::new("hello");
        field.set_cursor(0, 0);
        let window = 0..field.len();
        let _ = field.apply(
            Command::Motion(Motion::CharRight, Extend::Yes),
            window.clone(),
        );
        let _ = field.apply(
            Command::Motion(Motion::CharRight, Extend::Yes),
            window.clone(),
        );
        assert_eq!((field.cursor().anchor, field.cursor().position), (0, 2));
        assert_eq!(field.selected_text(), "he");

        // Unshifted left with an active selection collapses to the
        // selection's start instead of stepping one rune further left.
        let outcome = field.apply(
            Command::Motion(Motion::CharLeft, Extend::No),
            window.clone(),
        );
        assert_eq!(outcome, KeyOutcome::Consumed);
        assert_eq!(field.cursor().position, 0);
        assert!(!field.cursor().has_selection());
    }

    #[test]
    fn select_all_is_bounded_by_a_partial_window() {
        let mut field = TextField::new("lessrc.md");
        field.set_cursor(3, 3);
        let outcome = field.apply(Command::SelectAll, 0..6);
        assert_eq!(outcome, KeyOutcome::Consumed);
        assert_eq!((field.cursor().anchor, field.cursor().position), (0, 6));
        assert_eq!(field.selected_text(), "lessrc");
    }

    #[test]
    fn delete_left_on_a_multibyte_name_stays_on_char_boundaries() {
        let mut field = TextField::new("café");
        let window = 0..field.len();
        field.set_cursor(field.len(), field.len());
        let outcome = field.apply(Command::DeleteLeft, window);
        assert_eq!(outcome, KeyOutcome::Consumed);
        assert_eq!(field.text(), "caf");
        assert_eq!(field.cursor().position, 3);
    }

    #[test]
    fn an_edit_entirely_outside_the_window_is_a_no_op() {
        let mut field = TextField::new("ab.cd");
        field.set_cursor(5, 5);
        let outcome = field.apply(Command::DeleteLeft, 0..2);
        assert_eq!(outcome, KeyOutcome::Ignored);
        assert_eq!(field.text(), "ab.cd");
        assert_eq!(field.cursor().position, 5);
    }

    #[test]
    fn undo_then_redo_round_trips_a_typing_run_and_preserves_the_cursor_id() {
        let mut field = TextField::new("");
        let id = field.cursor().id;
        for ch in ["a", "b", "c"] {
            let window = 0..field.len();
            let outcome = field.insert(ch, window);
            assert_eq!(outcome, KeyOutcome::Consumed);
        }
        assert_eq!(field.text(), "abc");

        assert_eq!(
            field.apply(Command::Undo, 0..field.len()),
            KeyOutcome::Consumed
        );
        assert_eq!(
            field.apply(Command::Undo, 0..field.len()),
            KeyOutcome::Consumed
        );
        assert_eq!(field.text(), "a");

        assert_eq!(
            field.apply(Command::Redo, 0..field.len()),
            KeyOutcome::Consumed
        );
        assert_eq!(
            field.apply(Command::Redo, 0..field.len()),
            KeyOutcome::Consumed
        );
        assert_eq!(field.text(), "abc");
        assert_eq!(field.cursor().id, id);
    }

    #[test]
    fn set_text_clears_the_journal_so_a_following_undo_is_a_no_op() {
        let mut field = TextField::new("old");
        let window = 0..field.len();
        let _ = field.insert("!", window);
        field.set_text("new");
        let outcome = field.apply(Command::Undo, 0..field.len());
        assert_eq!(outcome, KeyOutcome::Ignored);
        assert_eq!(field.text(), "new");
    }

    #[test]
    fn is_empty_and_len_track_the_live_buffer() {
        let mut field = TextField::new("");
        assert!(field.is_empty());
        assert_eq!(field.len(), 0);
        let outcome = field.insert("hi", 0..0);
        assert_eq!(outcome, KeyOutcome::Consumed);
        assert!(!field.is_empty());
        assert_eq!(field.len(), 2);
    }
}
