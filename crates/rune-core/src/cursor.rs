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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub position: usize,
    pub anchor: usize,
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
#[path = "cursor_tests.rs"]
mod tests;
