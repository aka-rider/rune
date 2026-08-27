use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};

use crate::app::App;
use crate::focus::LayoutMode;
use crate::layout::{
    CENTER_LIMITS, DEFAULT_LEFT_PANE_W, FILESEARCH_MIN_W, MIN_CENTER_W, TABS_LIMITS,
    explorer_budget, explorer_fallback,
};
use crate::region::Region;

pub(crate) struct ColumnResolution {
    pub(crate) left_block: Option<Rect>,
    pub(crate) explorer_inner: Rect,
    pub(crate) tabs_divider: Option<Rect>,
    pub(crate) tabs_inner: Rect,
    pub(crate) center: Rect,
    pub(crate) mode: LayoutMode,
}

// `fits` is false only when neither section fits even alone — the
// caller's cue to give up on the column entirely rather than show a
// border around nothing.
struct ColumnCarve {
    explorer_inner: Rect,
    tabs_divider: Option<Rect>,
    tabs_inner: Rect,
    fits: bool,
}

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

// `split_fits` says whether the column's floor width fits beside the
// center pane's floor width.
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
        // Visibility and width are decided here, once; `app.splits` is
        // never written to reflect the finder's forced-visible column.
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
        // Below `split_fits` there's no room to paint the column beside
        // a center pane, so it becomes the whole frame instead of being
        // dropped — otherwise a finder opened on a narrow frame would
        // paint nothing while still consuming every keystroke.
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
            // Too SHORT, not just too narrow — give up on the column
            // entirely, matching the ordinary Split path's own no-fit case.
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
                    // Neither section fits even alone, so the column
                    // yields the space entirely and `center` reclaims it.
                    no_column
                }
            }
        }
    }
}
