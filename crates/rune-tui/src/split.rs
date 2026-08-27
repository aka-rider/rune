#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneLimits {
    pub min: u16,
    pub collapsible: bool,
}

// The fields are private so the invariant `desired >= limits.min` can never
// be broken from outside: "collapsed" has exactly one encoding (`shown ==
// false`), never confusable with "dragged very small" — which is also why
// a collapse preserves `desired` instead of clearing it.
#[derive(Clone, Copy, Debug)]
pub struct Split {
    limits: PaneLimits,
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

    // `allot` always returns `None` once `!shown`; this is the "as if
    // shown" query for a caller that overrides visibility for one frame.
    pub fn size_hint(&self, default: u16) -> u16 {
        self.desired.unwrap_or(default)
    }

    pub fn show(&mut self) {
        self.shown = true;
    }

    pub fn hide(&mut self) {
        if self.limits.collapsible {
            self.shown = false;
        }
    }

    // The comparison is `<`, not `<=`: a pane asked for exactly its floor is
    // shown at that floor. The non-collapsible branch below pins a pane to
    // `min` and expects it visible — with `<=` that state would be
    // unreachable, since the `< self.limits.min` branch would swallow the
    // boundary itself.
    pub fn request(&mut self, cells: u16) {
        if cells < self.limits.min {
            if self.limits.collapsible {
                self.shown = false;
            } else {
                self.desired = Some(self.limits.min);
                self.shown = true;
            }
        } else {
            self.desired = Some(cells);
            self.shown = true;
        }
    }

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

    // `available` must be the same axis length passed to `allot`. Writes
    // `desired` only when the trail is actually starved: an unconditional
    // write on a never-dragged split would permanently pin the trail to
    // exactly its floor the first time the user focuses it.
    pub fn ensure_trail(&mut self, available: u16, trail: PaneLimits) {
        if !self.shown {
            return;
        }
        let cap = available.saturating_sub(trail.min);
        let current = self.desired.unwrap_or(cap);
        if current <= cap {
            return;
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
        split.request(5);
        assert!(!split.is_shown());
        split.show();
        assert!(split.is_shown());
        assert_eq!(split.allot(120, 22, HORIZ_TRAIL), (Some(30), Some(90)));
    }

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
        let mut split = Split::new(VERT_LEAD, true);
        split.request(29);
        assert_eq!(split.allot(30, 3, VERT_TRAIL), (Some(30), None));
    }

    #[test]
    fn allot_vertical_shrink_past_a_dragged_desired_collapses_the_trail_transiently() {
        let mut split = Split::new(VERT_LEAD, true);
        split.request(25);
        assert_eq!(split.allot(10, 3, VERT_TRAIL), (Some(10), None));
        assert_eq!(split.allot(30, 3, VERT_TRAIL), (Some(25), Some(5)));
    }

    #[test]
    fn allot_never_dragged_shrink_degrades_proportionally_not_a_trail_collapse() {
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
        assert_eq!(split.allot(10, 20, trail), (Some(10), None));
    }

    #[test]
    fn ensure_trail_raises_starved_trail_to_floor() {
        let mut split = Split::new(VERT_LEAD, true);
        split.request(29);
        split.ensure_trail(30, VERT_TRAIL);
        assert_eq!(split.allot(30, 3, VERT_TRAIL), (Some(28), Some(2)));
    }

    #[test]
    fn ensure_trail_leaves_hidden_lead_untouched() {
        let mut split = Split::new(HORIZ_LEAD, true);
        split.request(5);
        assert!(!split.is_shown());
        split.ensure_trail(30, HORIZ_TRAIL);
        assert!(!split.is_shown());
    }

    #[test]
    fn ensure_trail_writes_nothing_when_trail_already_fits() {
        let split = Split::new(VERT_LEAD, true);
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
        dragged.request(5);
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
