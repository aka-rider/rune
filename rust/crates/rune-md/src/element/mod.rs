//! The element HSM hierarchy: reveal/conceal state machines and the
//! parent -> child reveal-inheritance carrier. Replaces Go's flat span list
//! and stateless `shouldReveal` (`pkg/editor/display/span_metadata.go:4-22`)
//! with a typed machine tree (plan Context, "The element HSM hierarchy").
//!
//! `doc` holds the root `DocMachine`/`DocState`/`WrapState`; `block` and
//! `inline` hold the leaf element machines. This module holds the shared
//! reveal vocabulary every one of them is built from.

pub mod block;
pub mod doc;
pub mod inline;

use crate::element::doc::{DocState, WrapState};
use rune_core::buffer::Buffer;
use rune_core::coords::BufferPoint;
use rune_core::cursor::CursorSet;

/// Whether a concealable element is showing its rendered (folded, styled)
/// form or its raw revealed markdown.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RevealState {
    #[default]
    Rendered,
    Revealed,
}

/// Embedded in every concealable element. `transition` is the ONLY writer of
/// `state` in the crate (`pkg/ui/components/image/image.go:191-243` pattern:
/// one `transition`, callers never assign the field directly). Returns true
/// iff the state actually changed, so callers can OR it into a dirty flag
/// without a separate equality check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RevealSm {
    state: RevealState,
}

impl RevealSm {
    pub fn new(initial: RevealState) -> RevealSm {
        RevealSm { state: initial }
    }

    pub fn state(&self) -> RevealState {
        self.state
    }

    /// The single writer of `state` for this machine kind. Every other
    /// machine in the crate reaches this through a method call
    /// (`self.sm.transition(..)`), never by assigning `.state` itself — the
    /// WP3 single-transition-writer test greps for the literal write and
    /// expects to find it exactly once, right here.
    pub fn transition(&mut self, next: RevealState) -> bool {
        if self.state == next {
            return false;
        }
        self.state = next;
        true
    }
}

/// Parent -> child reveal directive: the reveal-inheritance carrier. A
/// parent that is itself concealed (`Rendered`) forces every descendant
/// concealed too (`ForceRendered`); a parent that is revealed hands its
/// children `ForceRevealed` (an open bold span reveals its nested link as a
/// unit); `Decide` means "consult your own policy" — only ever handed out by
/// the document root when focused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealGrant {
    Decide,
    ForceRevealed,
    ForceRendered,
}

impl RevealGrant {
    /// The chokepoint every per-element Decide policy (the reveal-policy
    /// table in the plan Context) routes through: resolve a grant to a
    /// concrete `RevealState`, calling `decide` only when the grant itself
    /// doesn't already force an outcome. Keeps the three-armed
    /// `ForceRendered`/`ForceRevealed`/`Decide` match in exactly one place
    /// instead of copy-pasted into every machine's `sync`.
    pub fn resolve(self, decide: impl FnOnce() -> bool) -> RevealState {
        match self {
            RevealGrant::ForceRendered => RevealState::Rendered,
            RevealGrant::ForceRevealed => RevealState::Revealed,
            RevealGrant::Decide => {
                if decide() {
                    RevealState::Revealed
                } else {
                    RevealState::Rendered
                }
            }
        }
    }
}

/// A half-open, absolute-byte range: `[start, end)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

impl ByteRange {
    pub fn new(start: usize, end: usize) -> ByteRange {
        ByteRange { start, end }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    pub fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset < self.end
    }

    /// Clamp this range into `[0, len]`, keeping `start <= end`. Every range
    /// derived from comrak sourcepos or line arithmetic funnels through this
    /// before being stored on an element — the chokepoint that keeps
    /// `&content[a..b]` accesses downstream valid without sprinkling ad-hoc
    /// `min()` at every call site (Gotchas: "every `&content[a..b]` must come
    /// from validated/clamped ranges").
    pub fn clamp(&self, len: usize) -> ByteRange {
        let start = self.start.min(len);
        let end = self.end.min(len).max(start);
        ByteRange { start, end }
    }
}

/// Precomputed once per `sync_cursors` from `Buffer` + `CursorSet`: every
/// cursor's byte offset and its `BufferPoint` (line/col), so the reveal
/// policies (the `Decide` arm) never re-walk the cursor set or re-run
/// `offset_to_line_col` per element.
#[derive(Clone, Debug, Default)]
pub struct CursorProbe {
    offsets: Vec<usize>,
    points: Vec<BufferPoint>,
}

impl CursorProbe {
    pub fn new(buf: &Buffer, cursors: &CursorSet) -> CursorProbe {
        let offsets: Vec<usize> = cursors.all().iter().map(|c| c.position).collect();
        let points: Vec<BufferPoint> = offsets.iter().map(|&o| buf.offset_to_line_col(o)).collect();
        CursorProbe { offsets, points }
    }

    pub fn any_on_line(&self, line: usize) -> bool {
        self.points.iter().any(|p| p.line == line)
    }

    pub fn any_in(&self, r: ByteRange) -> bool {
        self.offsets.iter().any(|&o| r.contains(o))
    }

    pub fn any_in_lines(&self, first: usize, last: usize) -> bool {
        self.points
            .iter()
            .any(|p| p.line >= first && p.line <= last)
    }
}

/// The inherited context every parent hands its children. Downstream
/// elements NEVER own wrap or focus state — they read the root's through
/// this (plan directive: "downstream inherits upstream state").
#[derive(Clone, Copy)]
pub struct InheritCtx<'a> {
    pub focus: DocState,
    pub wrap: &'a WrapState,
    pub grant: RevealGrant,
    pub cursors: &'a CursorProbe,
}

impl<'a> InheritCtx<'a> {
    /// THE "downstream inherits upstream state" rule, in one function: a
    /// concealed (`Rendered`) parent forces every descendant concealed; a
    /// revealed parent forces its descendants revealed (nesting reveals as a
    /// unit); an already-forced grant from further up always wins.
    pub fn child(&self, own: RevealState) -> InheritCtx<'a> {
        let grant = match (self.grant, own) {
            (RevealGrant::ForceRendered, _) => RevealGrant::ForceRendered,
            (_, RevealState::Revealed) => RevealGrant::ForceRevealed,
            (g, RevealState::Rendered) => g,
        };
        InheritCtx { grant, ..*self }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn transition_reports_change() {
        let mut sm = RevealSm::new(RevealState::Rendered);
        assert!(!sm.transition(RevealState::Rendered));
        assert!(sm.transition(RevealState::Revealed));
        assert_eq!(sm.state(), RevealState::Revealed);
        assert!(!sm.transition(RevealState::Revealed));
    }

    #[test]
    fn child_forces_rendered_once_forced() {
        let wrap = WrapState { width: 80 };
        let cursors = CursorProbe::default();
        let ctx = InheritCtx {
            focus: DocState::Focused,
            wrap: &wrap,
            grant: RevealGrant::ForceRendered,
            cursors: &cursors,
        };
        let child = ctx.child(RevealState::Revealed);
        assert_eq!(child.grant, RevealGrant::ForceRendered);
    }

    #[test]
    fn child_forces_revealed_when_own_state_revealed() {
        let wrap = WrapState { width: 80 };
        let cursors = CursorProbe::default();
        let ctx = InheritCtx {
            focus: DocState::Focused,
            wrap: &wrap,
            grant: RevealGrant::Decide,
            cursors: &cursors,
        };
        let child = ctx.child(RevealState::Revealed);
        assert_eq!(child.grant, RevealGrant::ForceRevealed);
    }

    #[test]
    fn child_passes_through_grant_when_own_state_rendered() {
        let wrap = WrapState { width: 80 };
        let cursors = CursorProbe::default();
        let ctx = InheritCtx {
            focus: DocState::Focused,
            wrap: &wrap,
            grant: RevealGrant::Decide,
            cursors: &cursors,
        };
        let child = ctx.child(RevealState::Rendered);
        assert_eq!(child.grant, RevealGrant::Decide);
    }

    #[test]
    fn byte_range_clamp_keeps_start_le_end() {
        let r = ByteRange::new(5, 3).clamp(10);
        assert!(r.start <= r.end);
        let r2 = ByteRange::new(3, 20).clamp(10);
        assert_eq!(r2, ByteRange::new(3, 10));
    }
}
