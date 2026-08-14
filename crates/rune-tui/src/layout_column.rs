//! `layout::resolve`'s own left-column resolution — split out to keep
//! `layout.rs` under the 500-line budget: [`carve_column`] splits one
//! already-sized column block into its Explorer/divider/Tabs sections;
//! [`resolve_column`] decides the column's own visibility/width (and, at
//! the same time, the frame's `LayoutMode`) across its three cases
//! (filesearch-forced, narrow-frame full-width, ordinary side-by-side).

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};

use crate::app::App;
use crate::focus::LayoutMode;
use crate::layout::{
    CENTER_LIMITS, DEFAULT_LEFT_PANE_W, FILESEARCH_MIN_W, MIN_CENTER_W, TABS_LIMITS,
    explorer_budget, explorer_fallback,
};
use crate::region::Region;

/// The left column's rects plus the center pane and `LayoutMode`
/// `resolve_column` decides alongside it — each of its three branches
/// (filesearch-forced, narrow-frame full-width, ordinary side-by-side)
/// builds one of these.
pub(crate) struct ColumnResolution {
    pub(crate) left_block: Option<Rect>,
    pub(crate) explorer_inner: Rect,
    pub(crate) tabs_divider: Option<Rect>,
    pub(crate) tabs_inner: Rect,
    pub(crate) center: Rect,
    pub(crate) mode: LayoutMode,
}

/// [`carve_column`]'s own result: one already-sized left-column block split
/// into its Explorer/divider/Tabs sections. `fits` is `false` only when
/// NEITHER section fits even alone (`Split::allot`'s `(None, None)` arm) —
/// the caller's cue to give up on the column entirely rather than show a
/// border around nothing.
struct ColumnCarve {
    explorer_inner: Rect,
    tabs_divider: Option<Rect>,
    tabs_inner: Rect,
    fits: bool,
}

/// Splits one already-sized left-column block into its Explorer/divider/Tabs
/// sections — shared by the ordinary `Split` column and the narrow-frame
/// full-width `ExplorerOnly` column below, so the two can never diverge on
/// how a block of a given size divides.
fn carve_column(left_area: Rect, app: &App) -> ColumnCarve {
    let zero = Rect::new(0, 0, 0, 0);
    let inner = left_area.inner(Margin::new(1, 1));
    let budget = explorer_budget(left_area);
    let (explorer_h, tabs_h) =
        app.splits
            .explorer
            .allot(budget, explorer_fallback(left_area), TABS_LIMITS);

    match (explorer_h, tabs_h) {
        (Some(explorer_h), Some(tabs_h)) => {
            let explorer_inner = Region::carve_top(inner, explorer_h).rect();
            let divider = Region::row(inner, inner.y.saturating_add(explorer_h), 1).rect();
            let tabs_inner = Region::row(inner, divider.y.saturating_add(1), tabs_h).rect();
            ColumnCarve {
                explorer_inner,
                tabs_divider: Some(divider),
                tabs_inner,
                fits: true,
            }
        }
        (None, Some(_)) => {
            // The Explorer collapsed: the divider still labels the section
            // and sits at the very top of the inner rect, with the tab rows
            // filling everything below it.
            let divider = Region::row(inner, inner.y, 1).rect();
            let tabs_inner = Region::carve_bottom(inner, inner.height.saturating_sub(1)).rect();
            ColumnCarve {
                explorer_inner: zero,
                tabs_divider: Some(divider),
                tabs_inner,
                fits: true,
            }
        }
        (Some(_), None) => {
            // The tab rows collapsed: no divider at all, so the row
            // `explorer_budget` reserved for it comes back to the Explorer
            // — it takes the WHOLE inner height, not the `budget`-sized
            // number `allot` returned above (that number is measured
            // against `budget`, one row short of `inner.height`).
            ColumnCarve {
                explorer_inner: inner,
                tabs_divider: None,
                tabs_inner: zero,
                fits: true,
            }
        }
        (None, None) => ColumnCarve {
            explorer_inner: zero,
            tabs_divider: None,
            tabs_inner: zero,
            fits: false,
        },
    }
}

/// Decides the left column's own visibility/width and the frame's
/// `LayoutMode` — `layout::resolve`'s own sub-decision, split out here so
/// that file stays under the 500-line budget. `split_fits` is `layout::
/// resolve`'s own "does the column's floor fit beside the center's floor"
/// check.
pub(crate) fn resolve_column(main_area: Rect, split_fits: bool, app: &App) -> ColumnResolution {
    let zero = Rect::new(0, 0, 0, 0);
    let no_column = ColumnResolution {
        left_block: None,
        explorer_inner: zero,
        tabs_divider: None,
        tabs_inner: zero,
        center: main_area,
        mode: LayoutMode::EditorOnly,
    };

    if app.filesearch().is_some() && split_fits {
        // The finder forces the left column visible at its own width,
        // regardless of `app.splits.left.is_shown()` — visibility and
        // size are decided here, once, exactly like every other case
        // this function already handles; `app.splits` is never written.
        // Below `split_fits`, the finder falls through to the narrow-
        // frame handling below, unchanged.
        let cap = main_area.width.saturating_sub(MIN_CENTER_W);
        let filesearch_fits = main_area.width >= FILESEARCH_MIN_W.saturating_add(MIN_CENTER_W);
        let left_w = if filesearch_fits {
            app.splits
                .left
                .size_hint(DEFAULT_LEFT_PANE_W)
                .max(FILESEARCH_MIN_W)
                .min(cap)
        } else {
            cap
        };
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(left_w), Constraint::Min(0)])
            .split(main_area);
        let left_area = cols.first().copied().unwrap_or(main_area);
        let center = cols.get(1).copied().unwrap_or(main_area);

        let carve = carve_column(left_area, app);
        if carve.fits {
            ColumnResolution {
                left_block: Some(left_area),
                explorer_inner: carve.explorer_inner,
                tabs_divider: carve.tabs_divider,
                tabs_inner: carve.tabs_inner,
                center,
                mode: LayoutMode::Split {
                    explorer: carve.explorer_inner.height > 0,
                    tabs: carve.tabs_inner.height > 0,
                },
            }
        } else {
            no_column
        }
    } else if (app.splits.left.is_shown() || app.filesearch().is_some()) && !split_fits {
        // The finder falls through to this same narrow-frame branch as
        // an already-shown column: below `split_fits` there is no room
        // to paint the column beside a center pane, so — same as the
        // ordinary Explorer case — the column becomes the whole frame
        // rather than being dropped. Without this, a finder opened on
        // a frame narrower than `split_fits` would paint nothing at
        // all while still consuming every keystroke.
        let carve = carve_column(main_area, app);
        if carve.fits {
            // No `center` at all: the column IS the frame this mode.
            let center = Rect::new(
                main_area.x.saturating_add(main_area.width),
                main_area.y,
                0,
                main_area.height,
            );
            ColumnResolution {
                left_block: Some(main_area),
                explorer_inner: carve.explorer_inner,
                tabs_divider: carve.tabs_divider,
                tabs_inner: carve.tabs_inner,
                center,
                mode: LayoutMode::ExplorerOnly,
            }
        } else {
            // The column can't show anything even at full width (a
            // frame too SHORT, not just too narrow) — give up on it
            // exactly like the ordinary Split path's own `(None, None)`
            // arm does.
            no_column
        }
    } else {
        let (left_w, _trail) =
            app.splits
                .left
                .allot(main_area.width, DEFAULT_LEFT_PANE_W, CENTER_LIMITS);
        match left_w {
            None => no_column,
            Some(left_w) => {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(left_w), Constraint::Min(0)])
                    .split(main_area);
                let left_area = cols.first().copied().unwrap_or(main_area);
                let center = cols.get(1).copied().unwrap_or(main_area);

                let carve = carve_column(left_area, app);
                if carve.fits {
                    ColumnResolution {
                        left_block: Some(left_area),
                        explorer_inner: carve.explorer_inner,
                        tabs_divider: carve.tabs_divider,
                        tabs_inner: carve.tabs_inner,
                        center,
                        mode: LayoutMode::Split {
                            explorer: carve.explorer_inner.height > 0,
                            tabs: carve.tabs_inner.height > 0,
                        },
                    }
                } else {
                    // Neither section fits even alone: the column
                    // yields the space entirely rather than showing a
                    // border around nothing, and `center` reclaims the
                    // width the column would otherwise have reserved.
                    no_column
                }
            }
        }
    }
}
