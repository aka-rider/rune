//! Multi-cursor byte-offset primitives with anchor/position selection.
//! Ported from Go's cursor package. Phase 1 runs a single cursor;
//! `CursorSet` is the Go-parity seam for the multi-cursor future.

use crate::buffer::AppliedEdit;

/// One cursor: `position` is the head (blinks), `anchor` is the tail
/// (`position == anchor` means no selection). Port of `cursor.go:9-14`.
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

    pub fn collapse_to_start(&self) -> Cursor {
        let start = self.selection_start();
        Cursor {
            position: start,
            anchor: start,
            desired_col: self.desired_col,
            id: self.id,
        }
    }

    pub fn collapse_to_end(&self) -> Cursor {
        let end = self.selection_end();
        Cursor {
            position: end,
            anchor: end,
            desired_col: self.desired_col,
            id: self.id,
        }
    }
}

/// An ordered, non-overlapping set of cursors. Port of
/// `cursor.go:74-77`; `merge()` is the invariant-preserving chokepoint
/// every constructor and mutator routes through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CursorSet {
    cursors: Vec<Cursor>,
    next_id: u32,
}

impl CursorSet {
    /// Port of `cursor.go:79-84`.
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

    /// Port of `cursor.go:86-113`.
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

    /// Port of `cursor.go:115-128`.
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

    pub fn primary(&self) -> Cursor {
        self.cursors.first().copied().unwrap_or_default()
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

    /// Port of `cursor.go:151-166`.
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
    /// survivor. Port of `cursor.go:175-248`.
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

        let mut iter = cp.into_iter();
        let mut current = match iter.next() {
            Some(c) => c,
            None => {
                return CursorSet {
                    cursors: Vec::new(),
                    next_id: self.next_id,
                };
            }
        };
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

    pub fn map_with_index(&self, mut f: impl FnMut(usize, Cursor) -> Cursor) -> CursorSet {
        let cp: Vec<Cursor> = self
            .cursors
            .iter()
            .enumerate()
            .map(|(i, &c)| f(i, c))
            .collect();
        let res = CursorSet {
            cursors: cp,
            next_id: self.next_id,
        };
        res.merge()
    }

    /// Port of `cursor.go:274-289`: shift cursor offsets after a single
    /// `[start, end)` -> `insert_len`-byte replace.
    pub fn adjust_after_edit(&self, start: usize, end: usize, insert_len: usize) -> CursorSet {
        let net: isize = insert_len as isize - (end as isize - start as isize);
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

    /// Port of `cursor.go:291-317`: shift cursor offsets after a whole
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
                    shift += ae.insert.len() as isize - ae.deleted.len() as isize;
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
}
