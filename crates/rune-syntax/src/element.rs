use rune_core::buffer::Buffer;
use rune_core::coords::BufferPoint;
use rune_core::cursor::CursorSet;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RevealState {
    #[default]
    Rendered,
    Revealed,
}

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

    pub fn transition(&mut self, next: RevealState) -> bool {
        if self.state == next {
            return false;
        }
        self.state = next;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RevealGrant {
    Decide,
    ForceRevealed,
    ForceRendered,
}

impl RevealGrant {
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

    pub fn touches(&self, offset: usize) -> bool {
        offset >= self.start && offset <= self.end
    }

    pub fn clamp(&self, len: usize) -> ByteRange {
        let start = self.start.min(len);
        let end = self.end.min(len).max(start);
        ByteRange { start, end }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineLocal {
    line: usize,
    range: ByteRange,
}

impl LineLocal {
    pub fn clip(line: usize, bounds: Range<usize>, range: Range<usize>) -> Option<LineLocal> {
        if range.start > range.end || range.start < bounds.start || range.end > bounds.end {
            return None;
        }
        Some(LineLocal {
            line,
            range: ByteRange::new(range.start, range.end),
        })
    }

    pub fn line(&self) -> usize {
        self.line
    }

    pub fn range(&self) -> Range<usize> {
        self.range.start..self.range.end
    }

    pub fn start(&self) -> usize {
        self.range.start
    }

    pub fn end(&self) -> usize {
        self.range.end
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }
}

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

    pub fn touches(&self, r: ByteRange) -> bool {
        self.offsets.iter().any(|&o| r.touches(o))
    }

    pub fn any_in_lines(&self, first: usize, last: usize) -> bool {
        self.points
            .iter()
            .any(|p| p.line >= first && p.line <= last)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealMode {
    Never,
    AtCursor,
}

impl From<bool> for RevealMode {
    fn from(has_insertion_point: bool) -> RevealMode {
        if has_insertion_point {
            RevealMode::AtCursor
        } else {
            RevealMode::Never
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrapState {
    pub width: u16,
}

impl Default for WrapState {
    fn default() -> Self {
        WrapState { width: 80 }
    }
}

#[derive(Clone, Copy)]
pub struct InheritCtx<'a> {
    pub wrap: &'a WrapState,
    pub grant: RevealGrant,
    pub cursors: &'a CursorProbe,
}

impl<'a> InheritCtx<'a> {
    pub fn child(&self, own: RevealState) -> InheritCtx<'a> {
        let own_grant = match own {
            RevealState::Revealed => RevealGrant::ForceRevealed,
            RevealState::Rendered => self.grant,
        };
        let grant = self.grant.max(own_grant);
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
            wrap: &wrap,
            grant: RevealGrant::Decide,
            cursors: &cursors,
        };
        let child = ctx.child(RevealState::Rendered);
        assert_eq!(child.grant, RevealGrant::Decide);
    }

    #[test]
    fn resolve_force_rendered_ignores_the_decide_closure() {
        assert_eq!(
            RevealGrant::ForceRendered.resolve(|| true),
            RevealState::Rendered
        );
    }

    #[test]
    fn resolve_force_revealed_ignores_the_decide_closure() {
        assert_eq!(
            RevealGrant::ForceRevealed.resolve(|| false),
            RevealState::Revealed
        );
    }

    #[test]
    fn resolve_decide_defers_to_the_closure_result() {
        assert_eq!(RevealGrant::Decide.resolve(|| true), RevealState::Revealed);
        assert_eq!(RevealGrant::Decide.resolve(|| false), RevealState::Rendered);
    }

    #[test]
    fn byte_range_len_is_the_byte_span_not_a_constant() {
        assert_eq!(ByteRange::new(3, 10).len(), 7);
        assert_eq!(ByteRange::new(5, 5).len(), 0);
    }

    #[test]
    fn byte_range_contains_is_start_inclusive_end_exclusive() {
        let r = ByteRange::new(3, 7);
        assert!(!r.contains(2));
        assert!(r.contains(3));
        assert!(r.contains(6));
        assert!(!r.contains(7));
        assert!(!r.contains(8));
    }

    #[test]
    fn cursor_probe_any_on_line_matches_only_that_line() {
        let buf = Buffer::new("l0\nl1\nl2\nl3\nl4\nl5\nl6\n");
        let cursors = CursorSet::new(13); // the '4' in "l4"
        let probe = CursorProbe::new(&buf, &cursors);
        assert!(probe.any_on_line(4));
        assert!(!probe.any_on_line(3));
        assert!(!probe.any_on_line(5));
    }

    #[test]
    fn cursor_probe_any_in_lines_checks_an_inclusive_range() {
        let buf = Buffer::new("l0\nl1\nl2\nl3\nl4\nl5\nl6\n");
        let cursors = CursorSet::new(13); // the '4' in "l4"
        let probe = CursorProbe::new(&buf, &cursors);
        assert!(probe.any_in_lines(3, 5)); // 4 is strictly inside
        assert!(probe.any_in_lines(4, 6)); // 4 is the low boundary
        assert!(probe.any_in_lines(1, 4)); // 4 is the high boundary
        assert!(!probe.any_in_lines(5, 6)); // below the range
        assert!(!probe.any_in_lines(0, 3)); // above the range
    }

    #[test]
    fn cursor_probe_touches_includes_both_edges() {
        let buf = Buffer::new("0123456789");
        let range = ByteRange::new(3, 7);
        let at_start = CursorSet::new(3);
        let at_end = CursorSet::new(7);
        let past_end = CursorSet::new(8);
        assert!(CursorProbe::new(&buf, &at_start).touches(range));
        assert!(CursorProbe::new(&buf, &at_end).touches(range));
        assert!(!CursorProbe::new(&buf, &past_end).touches(range));
    }

    #[test]
    fn byte_range_touches_a_zero_length_range_at_its_own_offset() {
        assert!(ByteRange::new(3, 3).touches(3));
    }

    #[test]
    fn byte_range_clamp_keeps_start_le_end() {
        let r = ByteRange::new(5, 3).clamp(10);
        assert!(r.start <= r.end);
        let r2 = ByteRange::new(3, 20).clamp(10);
        assert_eq!(r2, ByteRange::new(3, 10));
    }

    #[test]
    fn line_local_clip_refuses_a_range_outside_bounds() {
        assert!(LineLocal::clip(3, 10..20, 5..25).is_none());
        assert!(LineLocal::clip(3, 10..20, 0..10).is_none());
        assert!(LineLocal::clip(3, 10..20, 25..30).is_none());
        let (inverted_start, inverted_end) = (15, 12);
        assert!(LineLocal::clip(3, 10..20, inverted_start..inverted_end).is_none());
    }

    #[test]
    fn line_local_clip_accepts_an_empty_range_inside_bounds() {
        let ll = LineLocal::clip(3, 10..20, 15..15).unwrap();
        assert!(ll.is_empty());
        assert_eq!(ll.line(), 3);
        assert_eq!(ll.start(), 15);
        assert_eq!(ll.end(), 15);
        assert_eq!(ll.range(), 15..15);
    }

    #[test]
    fn line_local_clip_accepts_a_range_touching_both_bounds() {
        let ll = LineLocal::clip(0, 10..20, 10..20).unwrap();
        assert!(!ll.is_empty());
        assert_eq!(ll.range(), 10..20);
    }
}
