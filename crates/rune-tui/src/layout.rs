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

/// Left-pane geometry: the DEFAULT column width, the smallest it may
/// shrink to, and the smallest the editor pane may shrink to in exchange.
/// `left_pane_width` below is the pure query both `geometry` and any test
/// asserting on the split use, so they can never disagree on the number.
pub const DEFAULT_LEFT_PANE_W: u16 = 22;
pub const MIN_LEFT_PANE_W: u16 = 16;
pub const MIN_CENTER_W: u16 = 24;

/// The left-pane column width for a `total_width`-wide main area, or `None`
/// when the terminal is too narrow to give BOTH the left pane its minimum
/// AND the editor `MIN_CENTER_W` (plan: "if the terminal is too narrow for
/// both minimums, drop the left pane for that frame").
pub fn left_pane_width(total_width: u16) -> Option<u16> {
    if total_width < MIN_LEFT_PANE_W.saturating_add(MIN_CENTER_W) {
        return None;
    }
    let max_left = total_width.saturating_sub(MIN_CENTER_W);
    Some(DEFAULT_LEFT_PANE_W.min(max_left).max(MIN_LEFT_PANE_W))
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
/// short to spare a row for it.
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
}

/// Pure — `&App` only, no `Frame`, no `&mut`. Computes, in order: (1) the
/// footer split; (2) the banner carve-out, if a modal is up; (3) the left
/// column's single bordered block and the Explorer/divider/Tabs split of
/// its inner rect, if `app.left_visible` and the terminal is wide enough;
/// (4) the remainder as `center`; (5) center's own title/editor split.
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
    let (left_block, explorer_inner, tabs_divider, tabs_inner, center) = if app.left_visible {
        match left_pane_width(main_area.width) {
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
                if inner.height < 2 {
                    // No room to spend a row on the divider — the Explorer
                    // takes what little there is and the Tabs section is
                    // simply absent this frame.
                    (Some(left_area), inner, None, zero, center)
                } else {
                    let rows = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Percentage(50),
                            Constraint::Length(1),
                            Constraint::Min(0),
                        ])
                        .split(inner);
                    (
                        Some(left_area),
                        rows.first().copied().unwrap_or(inner),
                        rows.get(1).copied(),
                        rows.get(2).copied().unwrap_or(zero),
                        center,
                    )
                }
            }
            None => (None, zero, None, zero, main_area),
        }
    } else {
        (None, zero, None, zero, main_area)
    };

    // The center pane gets a `Block::bordered()` (plan WP4.S1) whenever
    // there's room for the border to actually enclose something — matches
    // the same "too small, drop the chrome" shape `left_pane_width`/the old
    // `center_chrome_rows` used, just against the border's own 1-cell-per-
    // side minimum instead of a title+breadcrumb row minimum.
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
    }
}
