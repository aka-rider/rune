//! The pure pane-size model behind a user-draggable splitter: a pane has a
//! minimum size and a collapse policy, and when the size wanted falls below
//! that minimum, a collapsible pane disappears while a non-collapsible one
//! stays pinned at its floor. This module owns only that rule and the
//! allocator that applies it along one axis at a time; it draws nothing and
//! knows nothing about which axis (horizontal or vertical) it is being used
//! for — the same `Split` serves the left-column/editor split and the
//! Explorer/Tabs split within it.

/// One pane's floor on one axis, and whether it may vanish entirely rather
/// than be shown below that floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneLimits {
    pub min: u16,
    pub collapsible: bool,
}

/// One splitter's state: the pane's limits, the size the user last dragged
/// it to (if ever), and whether it is currently shown at all.
///
/// The fields are private so the invariant `desired >= limits.min` can never
/// be broken from outside: "collapsed" has exactly one encoding (`shown ==
/// false`), never confusable with "dragged very small". That is also why a
/// collapse preserves `desired` instead of clearing it — re-showing the pane
/// restores the size the user actually chose.
#[derive(Clone, Copy, Debug)]
pub struct Split {
    limits: PaneLimits,
    /// Where the user last dragged this splitter. `None` until they ever
    /// have — the pane then falls back to whatever default the frame
    /// suggests. Never below the floor: a smaller drag is recorded as
    /// "not shown" instead, so collapsing never loses the size to restore.
    desired: Option<u16>,
    shown: bool,
}

impl Split {
    pub const fn new(limits: PaneLimits, shown: bool) -> Split {
        Split {
            limits,
            desired: None,
            shown,
        }
    }

    pub fn limits(&self) -> PaneLimits {
        self.limits
    }

    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// The size `allot` would use for this pane if it were shown right
    /// now — `desired.unwrap_or(default)`, regardless of `self.shown`.
    /// `allot` itself always returns `None` outright once `!shown`, so this
    /// is the "as if shown" query a caller overriding visibility for one
    /// frame needs instead.
    pub fn size_hint(&self, default: u16) -> u16 {
        self.desired.unwrap_or(default)
    }

    /// Re-shows the pane without disturbing `desired` — a re-expose restores
    /// whatever size the user last dragged to, not some fresh default.
    pub fn show(&mut self) {
        self.shown = true;
    }

    /// Hides the pane, but only when its policy allows vanishing; otherwise
    /// a no-op, since a non-collapsible pane is never permitted to disappear.
    pub fn hide(&mut self) {
        if self.limits.collapsible {
            self.shown = false;
        }
    }

    /// The one place the user's rule lives: asked for a size below the
    /// floor, a collapsible pane vanishes (keeping `desired` intact for a
    /// later `show`); a non-collapsible pane instead gets pinned to `min`
    /// and stays shown, because it has nowhere else to go.
    ///
    /// The comparison is `<`, not `<=`: a pane asked for *exactly* its floor
    /// is shown at that floor. The non-collapsible branch below pins a pane
    /// to `min` and expects it visible — with `<=` that state ("sitting
    /// right at the floor, still shown") would be unreachable, since the
    /// `< self.limits.min` branch would swallow the boundary itself.
    pub fn request(&mut self, cells: u16) {
        if cells < self.limits.min {
            if self.limits.collapsible {
                self.shown = false; // `desired` is deliberately kept
            } else {
                self.desired = Some(self.limits.min);
                self.shown = true;
            }
        } else {
            self.desired = Some(cells);
            self.shown = true;
        }
    }

    /// Splits `available` cells on one axis between this pane (the "lead")
    /// and the pane that follows it (the "trail"), returning
    /// `(lead cells, trail cells)` with `None` meaning "not shown this
    /// frame". `fallback` is the size to use when the user has never
    /// dragged this splitter.
    pub fn allot(
        &self,
        available: u16,
        fallback: u16,
        trail: PaneLimits,
    ) -> (Option<u16>, Option<u16>) {
        if !self.shown {
            let trail_alloc = if available >= trail.min || !trail.collapsible {
                Some(available)
            } else {
                None
            };
            return (None, trail_alloc);
        }
        let lead = self.limits;

        // Neither floor fits: somebody has to go. A non-collapsible pane
        // always wins, and the LEAD is tested first so a non-collapsible
        // lead is never the one that vanishes. When both may go, the lead
        // is preferred.
        if available < lead.min.saturating_add(trail.min) {
            return match (lead.collapsible, trail.collapsible) {
                (false, _) => (Some(available), None),
                (_, false) => (None, Some(available)),
                (true, true) => {
                    if available >= lead.min {
                        (Some(available), None)
                    } else if available >= trail.min {
                        (None, Some(available))
                    } else {
                        (None, None)
                    }
                }
            };
        }

        // Both floors fit. A DRAGGED desired is honoured against the WHOLE
        // axis — not a cap that pre-reserves the trail's floor, which would
        // make the trail impossible to collapse by dragging. A never-dragged
        // split has expressed no such intent, so its fallback IS capped to
        // leave the trail its floor: a stale default computed for a wider
        // frame must yield room back on shrink rather than reading as a
        // deliberate push past the trail's boundary.
        let raw = self
            .desired
            .unwrap_or(fallback)
            .max(lead.min)
            .min(available);
        let want = if self.desired.is_some() {
            raw
        } else {
            raw.min(available.saturating_sub(trail.min))
        };
        let rest = available.saturating_sub(want);
        if rest >= trail.min {
            return (Some(want), Some(rest));
        }
        if trail.collapsible {
            // Nothing is left for the trail, so it goes and the lead takes
            // the whole axis. `want` is only ever capped for a never-dragged
            // split, so this arm is reached exclusively by a DRAGGED desired
            // pushing the boundary past the trail's floor — either straight
            // (`want` computed this call) or transiently, an axis that
            // shrank underneath a size an earlier drag recorded. Either way
            // `desired` is never written back here, so growing the axis
            // again restores both panes.
            return (Some(available), None);
        }
        // The trail may not vanish, so the lead yields back to its own
        // floor.
        let want = available.saturating_sub(trail.min); // >= lead.min, checked above
        (Some(want), Some(trail.min))
    }

    /// Shrinks the lead just enough that the trail gets its floor back.
    /// `available` MUST be the same axis length `allot` is called with — not
    /// some outer frame dimension. A no-op when the lead is hidden (the
    /// trail already owns the axis) and when the trail already fits, so it
    /// never writes `desired` gratuitously: an unconditional write on a
    /// never-dragged split would permanently pin the trail to exactly its
    /// floor the first time the user focuses it.
    pub fn ensure_trail(&mut self, available: u16, trail: PaneLimits) {
        if !self.shown {
            return;
        }
        let cap = available.saturating_sub(trail.min);
        let current = self.desired.unwrap_or(cap);
        if current <= cap {
            return; // the trail already has room; record nothing
        }
        self.request(cap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HORIZ_LEAD: PaneLimits = PaneLimits {
        min: 16,
        collapsible: true,
    };
    const HORIZ_TRAIL: PaneLimits = PaneLimits {
        min: 24,
        collapsible: false,
    };

    const VERT_LEAD: PaneLimits = PaneLimits {
        min: 3,
        collapsible: true,
    };
    const VERT_TRAIL: PaneLimits = PaneLimits {
        min: 2,
        collapsible: true,
    };

    #[test]
    fn request_below_floor_collapsible_hides_and_keeps_desired() {
        let mut split = Split::new(HORIZ_LEAD, true);
        split.request(20);
        assert_eq!(split.desired, Some(20));
        split.request(5);
        assert!(!split.is_shown());
        // `desired` survives the collapse so a later `show` restores it.
        assert_eq!(split.desired, Some(20));
    }

    #[test]
    fn request_below_floor_non_collapsible_pins_to_min_and_stays_shown() {
        let mut split = Split::new(HORIZ_TRAIL, true);
        split.request(5);
        assert!(split.is_shown());
        assert_eq!(split.desired, Some(HORIZ_TRAIL.min));
    }

    #[test]
    fn request_at_or_above_floor_shows_and_records() {
        let mut split = Split::new(HORIZ_LEAD, false);
        split.request(HORIZ_LEAD.min);
        assert!(split.is_shown());
        assert_eq!(split.desired, Some(HORIZ_LEAD.min));

        let mut split2 = Split::new(HORIZ_LEAD, false);
        split2.request(50);
        assert!(split2.is_shown());
        assert_eq!(split2.desired, Some(50));
    }

    #[test]
    fn show_after_collapse_restores_dragged_size_through_allot() {
        let mut split = Split::new(HORIZ_LEAD, true);
        split.request(30);
        split.request(5); // collapse, but `desired` stays 30
        assert!(!split.is_shown());
        split.show();
        assert!(split.is_shown());
        assert_eq!(split.allot(120, 22, HORIZ_TRAIL), (Some(30), Some(90)));
    }

    // Horizontal axis: pins the pre-drag fixed-width behaviour this
    // allocator reproduces — the same default width and floors the left
    // column always used before the divider became user-draggable.
    #[test]
    fn allot_horizontal_matches_the_old_fixed_left_width_query() {
        let never_dragged = Split::new(HORIZ_LEAD, true);
        assert_eq!(
            never_dragged.allot(120, 22, HORIZ_TRAIL),
            (Some(22), Some(98))
        );
        assert_eq!(
            never_dragged.allot(46, 22, HORIZ_TRAIL),
            (Some(22), Some(24))
        );
        assert_eq!(
            never_dragged.allot(40, 22, HORIZ_TRAIL),
            (Some(16), Some(24))
        );
        assert_eq!(never_dragged.allot(39, 22, HORIZ_TRAIL), (None, Some(39)));
        assert_eq!(never_dragged.allot(30, 22, HORIZ_TRAIL), (None, Some(30)));
    }

    #[test]
    fn allot_vertical_desired_28_fits_both() {
        let mut split = Split::new(VERT_LEAD, true);
        split.request(28);
        assert_eq!(split.allot(30, 3, VERT_TRAIL), (Some(28), Some(2)));
    }

    #[test]
    fn allot_vertical_desired_29_collapses_trail_not_capped() {
        // Guards against reintroducing the `available - trail.min` cap: a
        // capped allocator could never honour a `desired` past the trail's
        // floor, so it could never produce this collapse.
        let mut split = Split::new(VERT_LEAD, true);
        split.request(29);
        assert_eq!(split.allot(30, 3, VERT_TRAIL), (Some(30), None));
    }

    #[test]
    fn allot_vertical_shrink_past_a_dragged_desired_collapses_the_trail_transiently() {
        // An axis that shrinks below what an earlier drag asked for leaves
        // the trail nothing, so it collapses — the collapse rule applied to
        // a size the frame can no longer grant, not a special case. The
        // collapse is transient: `desired` is never written back, so an axis
        // restored to its old length restores both panes untouched. Pinned
        // because a "spare the trail on shrink" variant of this rule reads
        // as friendlier but silently costs the drag-to-collapse gesture,
        // whose natural overshoot then clamps to the floor instead.
        let mut split = Split::new(VERT_LEAD, true);
        split.request(25);
        assert_eq!(split.allot(10, 3, VERT_TRAIL), (Some(10), None));
        assert_eq!(split.allot(30, 3, VERT_TRAIL), (Some(25), Some(5)));
    }

    #[test]
    fn allot_never_dragged_shrink_degrades_proportionally_not_a_trail_collapse() {
        // The user never dragged this splitter, so `desired` is `None` and
        // `allot` is working only off `fallback` — a stale default computed
        // for a wider frame. Both floors (3 + 2) still fit inside the
        // shrunk axis (10), so the trail must stay visible: the fallback
        // yields back to the trail's floor instead of behaving like a real
        // drag that deliberately pushed past it.
        let split = Split::new(VERT_LEAD, true);
        assert_eq!(split.allot(10, 20, VERT_TRAIL), (Some(8), Some(2)));
    }

    #[test]
    fn allot_non_collapsible_lead_keeps_axis_when_neither_floor_fits() {
        let lead = PaneLimits {
            min: 20,
            collapsible: false,
        };
        let trail = PaneLimits {
            min: 20,
            collapsible: true,
        };
        let split = Split::new(lead, true);
        // available (10) is below both floors' sum (40); the non-collapsible
        // lead must keep the axis rather than vanish.
        assert_eq!(split.allot(10, 20, trail), (Some(10), None));
    }

    #[test]
    fn ensure_trail_raises_starved_trail_to_floor() {
        let mut split = Split::new(VERT_LEAD, true);
        split.request(29); // starves the trail, per the case above
        split.ensure_trail(30, VERT_TRAIL);
        assert_eq!(split.allot(30, 3, VERT_TRAIL), (Some(28), Some(2)));
    }

    #[test]
    fn ensure_trail_leaves_hidden_lead_untouched() {
        let mut split = Split::new(HORIZ_LEAD, true);
        split.request(5); // collapses (HORIZ_LEAD is collapsible)
        assert!(!split.is_shown());
        split.ensure_trail(30, HORIZ_TRAIL);
        assert!(!split.is_shown());
    }

    #[test]
    fn ensure_trail_writes_nothing_when_trail_already_fits() {
        let split = Split::new(VERT_LEAD, true); // never dragged
        let before = split.allot(30, 3, VERT_TRAIL);
        let mut after_split = split;
        after_split.ensure_trail(30, VERT_TRAIL);
        assert_eq!(after_split.allot(30, 3, VERT_TRAIL), before);
    }

    #[test]
    fn size_hint_reads_desired_or_falls_back_to_default_regardless_of_shown() {
        let never_dragged = Split::new(HORIZ_LEAD, false);
        assert_eq!(never_dragged.size_hint(22), 22);

        let mut dragged = Split::new(HORIZ_LEAD, true);
        dragged.request(30);
        assert_eq!(dragged.size_hint(22), 30);
        dragged.request(5); // collapses, but `desired` (30) survives
        assert!(!dragged.is_shown());
        assert_eq!(dragged.size_hint(22), 30);
    }

    #[test]
    fn allot_is_total_at_zero_and_one() {
        let horiz = Split::new(HORIZ_LEAD, true);
        let _ = horiz.allot(0, 22, HORIZ_TRAIL);
        let _ = horiz.allot(1, 22, HORIZ_TRAIL);
        let vert = Split::new(VERT_LEAD, true);
        let _ = vert.allot(0, 3, VERT_TRAIL);
        let _ = vert.allot(1, 3, VERT_TRAIL);

        let hidden_horiz = Split::new(HORIZ_LEAD, false);
        let _ = hidden_horiz.allot(0, 22, HORIZ_TRAIL);
        let _ = hidden_horiz.allot(1, 22, HORIZ_TRAIL);
    }
}
