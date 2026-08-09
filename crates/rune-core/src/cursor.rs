//! Multi-cursor byte-offset primitives with anchor/position selection.
//! Phase 1 runs a single cursor; `CursorSet` is built to grow into the
//! multi-cursor future.

use crate::assert_invariant;

/// One cursor: `position` is the head (blinks), `anchor` is the tail
/// (`position == anchor` means no selection).
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

/// An ordered, non-overlapping set of cursors. `merge()` is the
/// invariant-preserving chokepoint every constructor and mutator routes
/// through.
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
        assert_invariant!(!self.cursors.is_empty(), || {
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
    /// survivor.
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
        assert_invariant!(!cp.is_empty(), || {
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
