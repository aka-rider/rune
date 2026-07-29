//! Multi-cursor byte-offset primitives with anchor/position selection.
//! Ported from Go's cursor package. Phase 1 runs a single cursor;
//! `CursorSet` is the Go-parity seam for the multi-cursor future.

use crate::assert_invariant;
use crate::buffer::{AppliedEdit, edit_delta};

/// One cursor: `position` is the head (blinks), `anchor` is the tail
/// (`position == anchor` means no selection). Port of `cursor.go`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    /// Byte offset — the "head" (where the cursor blinks).
    pub position: usize,
    /// Byte offset — the "tail". Equals `position` when there is no
    /// selection.
    pub anchor: usize,
    /// Preserved column for vertical movement (Syntax Space).
    pub desired_col: usize,
    /// Stable identifier; never 0 for a real cursor (see
    /// `CursorSet::next_id`).
    pub id: u32,
}

impl Cursor {
    pub fn has_selection(&self) -> bool {
        self.position != self.anchor
    }

    pub fn selection_start(&self) -> usize {
        self.position.min(self.anchor)
    }

    pub fn selection_end(&self) -> usize {
        self.position.max(self.anchor)
    }

    pub fn selection_range(&self) -> (usize, usize) {
        if self.position < self.anchor {
            (self.position, self.anchor)
        } else {
            (self.anchor, self.position)
        }
    }

    pub fn reversed(&self) -> bool {
        self.position < self.anchor
    }

    pub fn collapse_to_position(&self) -> Cursor {
        Cursor {
            position: self.position,
            anchor: self.position,
            desired_col: self.desired_col,
            id: self.id,
        }
    }
}

/// An ordered, non-overlapping set of cursors. Port of
/// `cursor.go`; `merge()` is the invariant-preserving chokepoint
/// every constructor and mutator routes through.
///
/// Invariant: `cursors` is never empty — every public constructor produces
/// at least one cursor, and `merge` only ever coalesces cursors together,
/// never down to zero. A derived `#[derive(Default)]` would produce
/// `cursors: vec![]` with `next_id: 0` instead — the same malformed-empty
/// shape `Buffer`'s manual `Default` exists to prevent, and a `next_id` of
/// 0 would additionally hand the first cursor `add`s onto the set an id
/// of 0, colliding with `Cursor::id`'s own "no real cursor" meaning.
/// `CursorSet` gets a manual `Default` below that routes through
/// `CursorSet::new(0)` so no `CursorSet` can ever exist empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorSet {
    cursors: Vec<Cursor>,
    next_id: u32,
}

impl Default for CursorSet {
    fn default() -> Self {
        CursorSet::new(0)
    }
}

impl CursorSet {
    /// Port of `cursor.go`.
    pub fn new(offset: usize) -> CursorSet {
        CursorSet {
            cursors: vec![Cursor {
                position: offset,
                anchor: offset,
                desired_col: 0,
                id: 1,
            }],
            next_id: 2,
        }
    }

    /// Port of `cursor.go`.
    pub fn new_from(cursors: &[Cursor]) -> CursorSet {
        if cursors.is_empty() {
            return CursorSet::new(0);
        }
        let mut cp = cursors.to_vec();
        let mut max_id = 0u32;
        for c in &cp {
            if c.id > max_id {
                max_id = c.id;
            }
        }
        for c in &mut cp {
            if c.id == 0 {
                max_id += 1;
                c.id = max_id;
            }
        }
        let cs = CursorSet {
            cursors: cp,
            next_id: max_id + 1,
        };
        cs.merge()
    }

    /// Port of `cursor.go`.
    pub fn new_from_positions(positions: &[usize]) -> CursorSet {
        if positions.is_empty() {
            return CursorSet::new(0);
        }
        let cursors: Vec<Cursor> = positions
            .iter()
            .enumerate()
            .map(|(i, &p)| Cursor {
                position: p,
                anchor: p,
                desired_col: 0,
                id: (i as u32) + 1,
            })
            .collect();
        let cs = CursorSet {
            cursors,
            next_id: (positions.len() as u32) + 1,
        };
        cs.merge()
    }

    /// The first (lowest-`selection_start`) cursor. `cursors` is never
    /// empty (see the struct invariant above) — checked here rather than
    /// silently falling back to a defaulted `Cursor` (whose derived `id: 0`
    /// would collide with the "no real cursor" sentinel) so a future change
    /// that breaks the invariant is caught in tests instead of handing back
    /// a look-alike cursor.
    pub fn primary(&self) -> Cursor {
        assert_invariant(!self.cursors.is_empty(), || {
            "CursorSet::cursors must never be empty".to_string()
        });
        self.cursors.first().copied().unwrap_or(Cursor {
            position: 0,
            anchor: 0,
            desired_col: 0,
            id: 1,
        })
    }

    pub fn all(&self) -> Vec<Cursor> {
        self.cursors.clone()
    }

    pub fn len(&self) -> usize {
        self.cursors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cursors.is_empty()
    }

    pub fn is_multi(&self) -> bool {
        self.cursors.len() > 1
    }

    /// Port of `cursor.go`.
    pub fn add(&self, mut c: Cursor) -> CursorSet {
        let mut next_id = self.next_id;
        if c.id == 0 {
            c.id = next_id;
        }
        next_id += 1;
        let mut cp = self.cursors.clone();
        cp.push(c);
        let res = CursorSet {
            cursors: cp,
            next_id,
        };
        res.merge()
    }

    pub fn collapse_to(&self, primary: Cursor) -> CursorSet {
        CursorSet {
            cursors: vec![primary],
            next_id: self.next_id,
        }
    }

    /// Sort by `(selection_start, selection_end, id)`, then coalesce any
    /// cursors whose selections touch or overlap into their lower-id
    /// survivor. Port of `cursor.go`.
    pub fn merge(&self) -> CursorSet {
        if self.cursors.len() <= 1 {
            return self.clone();
        }

        let mut cp = self.cursors.clone();
        cp.sort_by(|a, b| {
            let (start_a, start_b) = (a.selection_start(), b.selection_start());
            if start_a != start_b {
                return start_a.cmp(&start_b);
            }
            let (end_a, end_b) = (a.selection_end(), b.selection_end());
            if end_a != end_b {
                return end_a.cmp(&end_b);
            }
            a.id.cmp(&b.id)
        });

        // `cp` holds `self.cursors.len()` elements and the guard above
        // already returned for `len() <= 1`, so at least 2 remain here —
        // `iter.next()` always yields `Some`.
        assert_invariant(!cp.is_empty(), || {
            "CursorSet::merge: cp must be non-empty past the len()<=1 guard".to_string()
        });
        let mut iter = cp.into_iter();
        let mut current = iter.next().unwrap_or(Cursor {
            position: 0,
            anchor: 0,
            desired_col: 0,
            id: 1,
        });
        let mut merged: Vec<Cursor> = Vec::new();

        for next in iter {
            if current.selection_end() >= next.selection_start() {
                let survivor_id = current.id.min(next.id);
                let start = current.selection_start();
                let mut end = current.selection_end();
                if next.selection_end() > end {
                    end = next.selection_end();
                }
                let is_reversed = if current.id == survivor_id {
                    current.reversed()
                } else {
                    next.reversed()
                };
                let (pos, anc) = if is_reversed {
                    (start, end)
                } else {
                    (end, start)
                };
                current = Cursor {
                    position: pos,
                    anchor: anc,
                    desired_col: current.desired_col,
                    id: survivor_id,
                };
            } else {
                merged.push(current);
                current = next;
            }
        }
        merged.push(current);

        CursorSet {
            cursors: merged,
            next_id: self.next_id,
        }
    }

    pub fn map(&self, mut f: impl FnMut(Cursor) -> Cursor) -> CursorSet {
        let cp: Vec<Cursor> = self.cursors.iter().map(|&c| f(c)).collect();
        let res = CursorSet {
            cursors: cp,
            next_id: self.next_id,
        };
        res.merge()
    }

    /// Port of `cursor.go`: shift cursor offsets after a single
    /// `[start, end)` -> `insert_len`-byte replace.
    pub fn adjust_after_edit(&self, start: usize, end: usize, insert_len: usize) -> CursorSet {
        let net: isize = edit_delta(end - start, insert_len);
        self.map(move |c| {
            let adjust = |pos: usize| -> usize {
                if pos < start {
                    pos
                } else if pos < end {
                    start + insert_len
                } else {
                    ((pos as isize) + net).max(0) as usize
                }
            };
            Cursor {
                position: adjust(c.position),
                anchor: adjust(c.anchor),
                ..c
            }
        })
    }

    /// Port of `cursor.go`: shift cursor offsets after a whole
    /// applied-edit batch. `edits` is in the same descending-`start` order
    /// `Buffer::apply_edits` returns; walking it from the last element
    /// (smallest `start`) to the first mirrors Go's `for i := len-1; i >=
    /// 0; i--`.
    pub fn adjust_after_batch_edits(&self, edits: &[AppliedEdit]) -> CursorSet {
        self.map(|c| {
            let adjust = |pos: usize| -> usize {
                let mut shift: isize = 0;
                for ae in edits.iter().rev() {
                    let old_start = ae.start as isize - shift;
                    let old_end = old_start + ae.deleted.len() as isize;
                    let pos_i = pos as isize;
                    if pos_i < old_start {
                        return (pos_i + shift).max(0) as usize;
                    }
                    if pos_i < old_end {
                        return ae.start + ae.insert.len();
                    }
                    shift += edit_delta(ae.deleted.len(), ae.insert.len());
                }
                (pos as isize + shift).max(0) as usize
            };
            Cursor {
                position: adjust(c.position),
                anchor: adjust(c.anchor),
                ..c
            }
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn new_single_cursor_has_id_one() {
        let cs = CursorSet::new(5);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs.primary().position, 5);
        assert_eq!(cs.primary().id, 1);
    }

    #[test]
    fn merge_coalesces_overlapping_selections() {
        let a = Cursor {
            position: 5,
            anchor: 0,
            desired_col: 0,
            id: 1,
        };
        let b = Cursor {
            position: 3,
            anchor: 8,
            desired_col: 0,
            id: 2,
        };
        let cs = CursorSet::new_from(&[a, b]);
        assert_eq!(cs.len(), 1);
        let merged = cs.primary();
        assert_eq!((merged.selection_start(), merged.selection_end()), (0, 8));
    }

    #[test]
    fn adjust_after_edit_shifts_positions_past_the_edit() {
        let cs = CursorSet::new(10);
        let adjusted = cs.adjust_after_edit(2, 4, 6);
        assert_eq!(adjusted.primary().position, 14); // 10 + (6 - 2)
    }

    /// [rune-core 14]: `adjust_after_batch_edits` is the one production
    /// path with the subtle pre-edit-coordinate reconstruction (each
    /// `AppliedEdit::start` already carries a cumulative shift baked in).
    /// Drive it from a REAL `apply_edits` output — not a hand-guessed
    /// `AppliedEdit` batch — so the test can't share a wrong assumption
    /// with the code under test: a cursor before, inside each edit, and
    /// past both.
    #[test]
    fn adjust_after_batch_edits_shifts_cursors_by_position() {
        use crate::buffer::{Buffer, Edit};

        let buf = Buffer::new("abcdefgh");
        let (new_buf, applied) = buf
            .apply_edits(&[
                Edit {
                    start: 6,
                    end: 8,
                    insert: String::new(),
                },
                Edit {
                    start: 2,
                    end: 4,
                    insert: "XYZ".to_string(),
                },
            ])
            .expect("edit should apply");
        assert_eq!(new_buf.content(), "abXYZef");

        let cs = CursorSet::new_from_positions(&[0, 3, 7, 8]);
        let adjusted = cs.adjust_after_batch_edits(&applied);
        let positions: Vec<usize> = adjusted.all().iter().map(|c| c.position).collect();
        // 0 precedes both edits: unchanged.
        // 3 falls inside the "cd" replace ([2,4)): snaps to that edit's
        // post-edit end (byte 5, right after "XYZ").
        // 7 falls inside the "gh" delete ([6,8)): snaps to that edit's
        // post-edit end (byte 7, the delete's own empty-insert end).
        // 8 (buffer's own length) follows both edits: shifted by both
        // deltas (+1 for "cd"->"XYZ", -2 for deleting "gh" — net -1), also
        // landing on byte 7 — `map`'s `merge()` then coalesces this
        // now-identical zero-width cursor with the previous one, so only
        // 3 cursors survive.
        assert_eq!(positions, vec![0, 5, 7]);
        assert_eq!(adjusted.len(), 3);
    }

    /// [rune-core 14]: when two overlapping cursors merge, the survivor's
    /// `reversed()` flag must come from whichever of the two carries the
    /// surviving (lower) id — not always the earlier-sorted cursor.
    #[test]
    fn merge_survivor_keeps_the_reversed_flag_of_the_lower_id_cursor() {
        // `a` (id 1, survivor) is NOT reversed: position is its selection end.
        let a = Cursor {
            position: 8,
            anchor: 0,
            desired_col: 0,
            id: 1,
        };
        // `b` (id 2) IS reversed and sorts first by selection_start.
        let b = Cursor {
            position: 3,
            anchor: 6,
            desired_col: 0,
            id: 2,
        };
        let merged = CursorSet::new_from(&[a, b]).primary();
        assert_eq!(merged.id, 1, "lower id survives");
        assert!(
            !merged.reversed(),
            "the survivor's own reversed flag (id 1, not reversed) must win, \
             not the other cursor's"
        );
        assert_eq!((merged.selection_start(), merged.selection_end()), (0, 8));
    }

    /// [rune-core 14]: `new_from` assigns fresh ids to any cursor with
    /// `id == 0`, past the highest id already present.
    #[test]
    fn new_from_assigns_fresh_ids_to_zero_id_cursors() {
        let a = Cursor {
            position: 0,
            anchor: 0,
            desired_col: 0,
            id: 5,
        };
        let b = Cursor {
            position: 20,
            anchor: 20,
            desired_col: 0,
            id: 0,
        };
        let cs = CursorSet::new_from(&[a, b]);
        assert_eq!(cs.len(), 2);
        let ids: Vec<u32> = cs.all().iter().map(|c| c.id).collect();
        assert!(ids.contains(&5));
        assert!(
            ids.iter().all(|&id| id != 0),
            "id 0 must be reassigned: {ids:?}"
        );
    }

    /// [rune-core 14]: `new_from` does not itself deduplicate ids — two
    /// cursors sharing a non-zero id both survive `new_from` unless their
    /// selections happen to touch (`merge`'s job, not id assignment's).
    #[test]
    fn new_from_does_not_dedupe_non_touching_duplicate_ids() {
        let a = Cursor {
            position: 0,
            anchor: 0,
            desired_col: 0,
            id: 7,
        };
        let b = Cursor {
            position: 50,
            anchor: 50,
            desired_col: 0,
            id: 7,
        };
        let cs = CursorSet::new_from(&[a, b]);
        assert_eq!(cs.len(), 2, "non-touching cursors are not merged by id");
        let ids: Vec<u32> = cs.all().iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![7, 7]);
    }
}
