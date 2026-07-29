//! The ONE geometry chokepoint (plan Context, "The root cause underneath
//! both bugs"): a pure function from `(area, &App)` to every rect the frame
//! is built from. `render::draw`, `App::relayout` (which sizes the active
//! document's viewport from it), and `explorer`/`opentabs`'s own
//! `visible_rows` all read from here — before this module existed, all
//! three independently reverse-engineered their own idea of the editor's
//! rect from `viewport.height + 1`, and could silently disagree the moment
//! a border was added. `geometry` itself never touches a `Frame` and never
//! takes `&mut App` — it is safe to call as often as any of its three
//! readers like, always with the same answer for the same inputs.
//!
//! A Rust port of Go's `paneGeometry()`.

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};

use crate::app::App;
use crate::banner;
use crate::split::{PaneLimits, Split};

/// Left-pane geometry: the DEFAULT column width, and the smallest the left
/// column and the editor pane may each shrink to before one of them gives
/// way. These feed `LEFT_LIMITS`/`CENTER_LIMITS` below, which `Split::allot`
/// reads instead of any bespoke width query, so there is exactly one place
/// that knows the numbers.
pub const DEFAULT_LEFT_PANE_W: u16 = 22;
pub const MIN_LEFT_PANE_W: u16 = 16;
pub const MIN_CENTER_W: u16 = 24;

/// Inner rows, not block rows: the sections share one border, so these are
/// measured on the block's inner rect. The Explorer spends one row on a
/// header, so its floor leaves a header plus two entries.
pub const MIN_EXPLORER_H: u16 = 3;
pub const MIN_TABS_H: u16 = 2;

pub const LEFT_LIMITS: PaneLimits = PaneLimits {
    min: MIN_LEFT_PANE_W,
    collapsible: true,
};
pub const CENTER_LIMITS: PaneLimits = PaneLimits {
    min: MIN_CENTER_W,
    collapsible: false,
};
pub const EXPLORER_LIMITS: PaneLimits = PaneLimits {
    min: MIN_EXPLORER_H,
    collapsible: true,
};
pub const TABS_LIMITS: PaneLimits = PaneLimits {
    min: MIN_TABS_H,
    collapsible: true,
};

/// The two draggable splitter positions. These are DESIRED sizes: geometry
/// clamps them against the live frame every call and never writes back, so
/// narrowing the terminal and widening it again restores exactly what the
/// user dragged to.
#[derive(Clone, Copy, Debug)]
pub struct Splits {
    /// Left-column width; the Explorer and the tab rows share it.
    pub left: Split,
    /// The Explorer's height inside the column's inner rect; the tab rows
    /// take what is left after the one-row divider.
    pub explorer: Split,
}

impl Default for Splits {
    fn default() -> Splits {
        Splits {
            left: Split::new(LEFT_LIMITS, false),
            explorer: Split::new(EXPLORER_LIMITS, true),
        }
    }
}

/// The rows the Explorer/Open split is allotted from: the column block's
/// inner height, less the one row the divider between them costs.
pub fn explorer_budget(block: Rect) -> u16 {
    block.height.saturating_sub(2).saturating_sub(1)
}

/// The Explorer's share until the user ever drags the divider: half the
/// column's inner rect, rounded up. Reproduces the constraint solver's
/// answer for the even split this replaced.
pub fn explorer_fallback(block: Rect) -> u16 {
    block.height.saturating_sub(2).div_ceil(2)
}

/// Every rect `render::draw` blits into, computed once, read by every
/// consumer that used to guess its own.
///
/// The left column is ONE bordered block (`left_block`), not two stacked
/// ones: the Explorer's rows fill its upper half (`explorer_inner`), a
/// one-row in-block `Open` divider (`tabs_divider`) introduces the Open
/// Tabs section, and the tab rows fill the rest (`tabs_inner`). All three
/// are carved out of the block's single inner rect, so no interior border
/// rule ever splits the column.
///
/// `explorer_inner`/`tabs_inner` are `Rect`, not `Option<Rect>` — when the
/// left pane isn't shown at all (`left_block` `None`), they're the zero
/// rect `Rect::new(0, 0, 0, 0)`; nothing reads them in that state except
/// `explorer`/`opentabs`'s own `visible_rows`, whose callers only run while
/// the corresponding pane can actually be focused, i.e. while it's visible.
/// `tabs_divider` is additionally `None` when the block's inner rect is too
/// short to spare a row for it, or when the Explorer's own floor doesn't
/// leave the tab rows theirs.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub footer: Rect,
    pub banner: Option<Rect>,
    pub left_block: Option<Rect>,
    pub explorer_inner: Rect,
    pub tabs_divider: Option<Rect>,
    pub tabs_inner: Rect,
    pub center: Rect,
    /// Whether the center pane got a `Block::bordered()` this frame (plan
    /// WP4: `center.width >= 3 && center.height >= 3`). `false` throughout
    /// WP3 — no border exists yet, so `render::draw` never asks for one.
    pub center_bordered: bool,
    pub title: Option<Rect>,
    pub editor: Rect,
    /// The area left after the footer and any banner are carved out — the
    /// height the left column is allotted from.
    pub main: Rect,
    /// The two-cell-wide band the user grabs to resize the left column: the
    /// column block's right border plus the centre block's left border.
    /// `None` when the column isn't showing. The VERTICAL grab band needs
    /// no field of its own — it is exactly `tabs_divider`.
    pub left_splitter: Option<Rect>,
}

/// Pure — `&App` only, no `Frame`, no `&mut`. Computes, in order: (1) the
/// footer split; (2) the banner carve-out, if a modal is up; (3) the left
/// column's single bordered block and the Explorer/divider/Tabs split of
/// its inner rect, sized by `app.splits`; (4) the remainder as `center`;
/// (5) center's own title/editor split.
///
/// Every subtraction saturates — `geometry` never panics, however small
/// `area` is (the fuzzer drives `Resize` down to a 1-column, 2-row
/// terminal, and the render tests draw at `(0, 0)` and `(1, 1)`).
pub fn geometry(area: Rect, app: &App) -> Geometry {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    let main_area = chunks.first().copied().unwrap_or(area);
    let footer = chunks
        .get(1)
        .copied()
        .unwrap_or(Rect::new(area.x, area.y, area.width, 0));

    let (main_area, banner) = if app.modal.is_some() {
        let banner_h = banner::banner_height(app, area.height);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(banner_h)])
            .split(main_area);
        (
            rows.first().copied().unwrap_or(main_area),
            rows.get(1).copied(),
        )
    } else {
        (main_area, None)
    };

    let zero = Rect::new(0, 0, 0, 0);
    let (left_w, _trail) =
        app.splits
            .left
            .allot(main_area.width, DEFAULT_LEFT_PANE_W, CENTER_LIMITS);

    let (left_block, explorer_inner, tabs_divider, tabs_inner, center) = match left_w {
        None => (None, zero, None, zero, main_area),
        Some(left_w) => {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(left_w), Constraint::Min(0)])
                .split(main_area);
            let left_area = cols.first().copied().unwrap_or(main_area);
            let center = cols.get(1).copied().unwrap_or(main_area);

            // The ONE border's inner rect, carved once: everything the
            // left column draws lives inside it, so the Explorer rows,
            // the divider and the tab rows can never disagree about
            // where the border is.
            let inner = left_area.inner(Margin::new(1, 1));
            let budget = explorer_budget(left_area);
            let (explorer_h, tabs_h) =
                app.splits
                    .explorer
                    .allot(budget, explorer_fallback(left_area), TABS_LIMITS);

            match (explorer_h, tabs_h) {
                (Some(explorer_h), Some(tabs_h)) => {
                    let explorer_inner = Rect::new(inner.x, inner.y, inner.width, explorer_h);
                    let divider =
                        Rect::new(inner.x, inner.y.saturating_add(explorer_h), inner.width, 1);
                    let tabs_inner =
                        Rect::new(inner.x, divider.y.saturating_add(1), inner.width, tabs_h);
                    (
                        Some(left_area),
                        explorer_inner,
                        Some(divider),
                        tabs_inner,
                        center,
                    )
                }
                (None, Some(_)) => {
                    // The Explorer collapsed: the divider still labels the
                    // section and sits at the very top of the inner rect,
                    // with the tab rows filling everything below it.
                    let divider = Rect::new(inner.x, inner.y, inner.width, 1);
                    let tabs_inner = Rect::new(
                        inner.x,
                        inner.y.saturating_add(1),
                        inner.width,
                        inner.height.saturating_sub(1),
                    );
                    (Some(left_area), zero, Some(divider), tabs_inner, center)
                }
                (Some(_), None) => {
                    // The tab rows collapsed: no divider at all, so the row
                    // `explorer_budget` reserved for it comes back to the
                    // Explorer — it takes the WHOLE inner height, not the
                    // `budget`-sized number `allot` returned above (that
                    // number is measured against `budget`, one row short of
                    // `inner.height`).
                    (Some(left_area), inner, None, zero, center)
                }
                (None, None) => {
                    // Neither section fits even alone: the column yields
                    // the space entirely rather than showing a border
                    // around nothing, and `center` reclaims the width the
                    // column would otherwise have reserved.
                    (None, zero, None, zero, main_area)
                }
            }
        }
    };

    // Derived from `left_block`, never from `left_w`: the two disagree
    // exactly in the both-sections-collapsed case above, where the
    // horizontal allot succeeded but the column still shows nothing.
    let left_splitter = left_block.map(|b| {
        let max_x = main_area
            .x
            .saturating_add(main_area.width)
            .saturating_sub(2);
        let x = b.x.saturating_add(b.width).saturating_sub(1).min(max_x);
        Rect::new(x, main_area.y, 2, main_area.height)
    });

    // The center pane gets a `Block::bordered()` (plan WP4.S1) whenever
    // there's room for the border to actually enclose something — matches
    // the same "too small, drop the chrome" shape the left column's own
    // width floor used, just against the border's own 1-cell-per-side
    // minimum instead of a title+breadcrumb row minimum.
    let center_bordered = center.width >= 3 && center.height >= 3;
    let content = if center_bordered {
        center.inner(Margin::new(1, 1))
    } else {
        center
    };

    // No more breadcrumb rect (plan WP4.S1): the breadcrumb is spliced
    // directly onto the bordered block's own bottom border row
    // (`breadcrumb::overlay`, reading `geo.center` + `geo.center_bordered`
    // itself) rather than reserving a second content row the way the
    // pre-WP4 `center_chrome_rows` did.
    let title = (content.height >= 1).then(|| Rect::new(content.x, content.y, content.width, 1));
    let editor = Rect::new(
        content.x,
        content.y.saturating_add(1),
        content.width,
        content.height.saturating_sub(1),
    );

    Geometry {
        footer,
        banner,
        left_block,
        explorer_inner,
        tabs_divider,
        tabs_inner,
        center,
        center_bordered,
        title,
        editor,
        main: main_area,
        left_splitter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `explorer_budget` is a public function precisely because `geometry`
    /// and `pane::FocusTabs`'s `ensure_trail` call must both use the exact
    /// same quantity — pin that it agrees with the inner rect `geometry`
    /// itself carves the vertical split from, rather than trusting the two
    /// expressions to stay in sync by inspection.
    #[test]
    fn explorer_budget_matches_the_inner_rect_geometry_actually_splits() {
        for left_area in [
            Rect::new(0, 0, 22, 33),
            Rect::new(0, 0, 22, 23),
            Rect::new(0, 0, 22, 24),
        ] {
            let inner = left_area.inner(Margin::new(1, 1));
            assert_eq!(explorer_budget(left_area), inner.height.saturating_sub(1));
        }
    }
}
