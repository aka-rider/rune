//! Multi-cursor byte-offset primitives with anchor/position selection.
//! Phase 1 runs a single cursor; `CursorSet` is built to grow into the
//! multi-cursor future.

use std::fmt;
use std::num::NonZeroU32;

use crate::assert_invariant;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CursorId(NonZeroU32);

impl CursorId {
    pub const FIRST: CursorId = CursorId(NonZeroU32::MIN);

    pub fn get(self) -> u32 {
        self.0.get()
    }

    fn next(self) -> CursorId {
        CursorId(self.0.saturating_add(1))
    }
}

impl fmt::Display for CursorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorIdZero;

impl fmt::Display for CursorIdZero {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cursor id must be non-zero")
    }
}

impl std::error::Error for CursorIdZero {}

impl TryFrom<u32> for CursorId {
    type Error = CursorIdZero;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        NonZeroU32::new(value).map(CursorId).ok_or(CursorIdZero)
    }
}

/// One cursor: `position` is the head (blinks), `anchor` is the tail
/// (`position == anchor` means no selection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// Byte offset — the "head" (where the cursor blinks).
    pub position: usize,
    /// Byte offset — the "tail". Equals `position` when there is no
    /// selection.
    pub anchor: usize,
    /// Preserved column for vertical movement (Syntax Space).
    pub desired_col: usize,
    pub id: CursorId,
}

impl Cursor {
    const FALLBACK: Cursor = Cursor {
        position: 0,
        anchor: 0,
        desired_col: 0,
        id: CursorId::FIRST,
    };

    fn from_spec(spec: CursorSpec, id: CursorId) -> Cursor {
        Cursor {
            position: spec.position,
            anchor: spec.anchor,
            desired_col: spec.desired_col,
            id,
        }
    }

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CursorSpec {
    pub position: usize,
    pub anchor: usize,
    pub desired_col: usize,
}

/// An ordered, non-overlapping set of cursors. `merge()` is the
/// invariant-preserving chokepoint every constructor and mutator routes
/// through.
///
/// Invariant: `cursors` is never empty — every public constructor produces
/// at least one cursor, and `merge` only ever coalesces cursors together,
/// never down to zero. A derived `#[derive(Default)]` would produce
/// `cursors: vec![]`, so `CursorSet` gets a manual `Default` below that
/// routes through `CursorSet::new(0)` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorSet {
    cursors: Vec<Cursor>,
    next_id: CursorId,
}

impl Default for CursorSet {
    fn default() -> Self {
        CursorSet::new(0)
    }
}

impl CursorSet {
    pub fn new(offset: usize) -> CursorSet {
        CursorSet::new_from_specs(&[CursorSpec {
            position: offset,
            anchor: offset,
            desired_col: 0,
        }])
    }

    pub fn new_from(cursors: &[Cursor]) -> CursorSet {
        if cursors.is_empty() {
            return CursorSet::new(0);
        }
        let mut next_id = CursorId::FIRST;
        for c in cursors {
            if c.id >= next_id {
                next_id = c.id.next();
            }
        }
        let cs = CursorSet {
            cursors: cursors.to_vec(),
            next_id,
        };
        cs.merge()
    }

    pub fn new_from_specs(specs: &[CursorSpec]) -> CursorSet {
        if specs.is_empty() {
            return CursorSet::new(0);
        }
        let mut next_id = CursorId::FIRST;
        let cursors: Vec<Cursor> = specs
            .iter()
            .map(|s| {
                let cursor = Cursor::from_spec(*s, next_id);
                next_id = next_id.next();
                cursor
            })
            .collect();
        let cs = CursorSet { cursors, next_id };
        cs.merge()
    }

    pub fn new_from_positions(positions: &[usize]) -> CursorSet {
        let specs: Vec<CursorSpec> = positions
            .iter()
            .map(|&p| CursorSpec {
                position: p,
                anchor: p,
                desired_col: 0,
            })
            .collect();
        CursorSet::new_from_specs(&specs)
    }

    /// The first (lowest-`selection_start`) cursor. `cursors` is never
    /// empty (see the struct invariant above) — checked here rather than
    /// silently falling back to a look-alike cursor, so a future change
    /// that breaks the invariant is caught in tests.
    pub fn primary(&self) -> Cursor {
        assert_invariant!(!self.cursors.is_empty(), || {
            "CursorSet::cursors must never be empty".to_string()
        });
        self.cursors.first().copied().unwrap_or(Cursor::FALLBACK)
    }

    pub fn all(&self) -> &[Cursor] {
        &self.cursors
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

    pub fn add(&self, spec: CursorSpec) -> CursorSet {
        let cursor = Cursor::from_spec(spec, self.next_id);
        let mut cp = self.cursors.clone();
        cp.push(cursor);
        let res = CursorSet {
            cursors: cp,
            next_id: self.next_id.next(),
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
    /// survivor. The survivor carries the merged id and column, but the
    /// merged DIRECTION comes from a cursor that actually has a selection:
    /// an empty cursor is never `reversed()`, so letting an empty survivor
    /// decide would face the result downward and strand the caret at the
    /// far end of a selection the user built by reaching up.
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
        let mut current = iter.next().unwrap_or(Cursor::FALLBACK);
        let mut merged: Vec<Cursor> = Vec::new();

        for next in iter {
            if current.selection_end() >= next.selection_start() {
                let survivor_id = current.id.min(next.id);
                let start = current.selection_start();
                let mut end = current.selection_end();
                if next.selection_end() > end {
                    end = next.selection_end();
                }
                let survivor = if current.id == survivor_id {
                    current
                } else {
                    next
                };
                let is_reversed = if survivor.has_selection() {
                    survivor.reversed()
                } else if current.has_selection() {
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

    fn id(n: u32) -> CursorId {
        CursorId::try_from(n).expect("test ids are non-zero")
    }

    #[test]
    fn new_single_cursor_has_id_one() {
        let cs = CursorSet::new(5);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs.primary().position, 5);
        assert_eq!(cs.primary().id, CursorId::FIRST);
    }

    #[test]
    fn merge_coalesces_overlapping_selections() {
        let a = Cursor {
            position: 5,
            anchor: 0,
            desired_col: 0,
            id: id(1),
        };
        let b = Cursor {
            position: 3,
            anchor: 8,
            desired_col: 0,
            id: id(2),
        };
        let cs = CursorSet::new_from(&[a, b]);
        assert_eq!(cs.len(), 1);
        let merged = cs.primary();
        assert_eq!((merged.selection_start(), merged.selection_end()), (0, 8));
    }

    /// An empty cursor has no direction — `reversed()` is false for it
    /// however the user got there. Taking the merged direction from an
    /// empty survivor therefore flips a real selection to face the wrong
    /// way: pressing Up merges a clamped-at-top empty cursor with a
    /// selection reaching up to it, and the head must stay at the TOP.
    #[test]
    fn merge_takes_its_direction_from_the_cursor_that_has_a_selection() {
        let clamped_at_top = Cursor {
            position: 0,
            anchor: 0,
            desired_col: 0,
            id: id(1),
        };
        let reaching_up = Cursor {
            position: 0,
            anchor: 8,
            desired_col: 0,
            id: id(2),
        };
        let cs = CursorSet::new_from(&[clamped_at_top, reaching_up]);
        assert_eq!(cs.len(), 1);
        let merged = cs.primary();
        assert_eq!((merged.selection_start(), merged.selection_end()), (0, 8));
        assert_eq!(
            merged.position, 0,
            "merging an empty cursor with a selection reaching up must keep the head at the top"
        );
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
            id: id(1),
        };
        // `b` (id 2) IS reversed and sorts first by selection_start.
        let b = Cursor {
            position: 3,
            anchor: 6,
            desired_col: 0,
            id: id(2),
        };
        let merged = CursorSet::new_from(&[a, b]).primary();
        assert_eq!(merged.id, id(1), "lower id survives");
        assert!(
            !merged.reversed(),
            "the survivor's own reversed flag (id 1, not reversed) must win, \
             not the other cursor's"
        );
        assert_eq!((merged.selection_start(), merged.selection_end()), (0, 8));
    }

    #[test]
    fn new_from_specs_assigns_distinct_fresh_ids() {
        let specs = [
            CursorSpec {
                position: 0,
                anchor: 0,
                desired_col: 0,
            },
            CursorSpec {
                position: 20,
                anchor: 20,
                desired_col: 0,
            },
        ];
        let cs = CursorSet::new_from_specs(&specs);
        assert_eq!(cs.len(), 2);
        let ids: Vec<CursorId> = cs.all().iter().map(|c| c.id).collect();
        assert_ne!(ids[0], ids[1], "each spec gets a distinct id");
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
            id: id(7),
        };
        let b = Cursor {
            position: 50,
            anchor: 50,
            desired_col: 0,
            id: id(7),
        };
        let cs = CursorSet::new_from(&[a, b]);
        assert_eq!(cs.len(), 2, "non-touching cursors are not merged by id");
        let ids: Vec<CursorId> = cs.all().iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![id(7), id(7)]);
    }
}
