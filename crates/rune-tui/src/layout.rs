//! The ONE geometry chokepoint: a pure function from `(area, &App)` to every rect the frame is
//! built from. The frame renderer, `App::relayout` (which sizes the active document's viewport from
//! it), and `explorer`/`opentabs`'s own `visible_rows` all read from here — before this module
//! existed, all three independently reverse-engineered their own idea of the editor's rect from
//! `viewport.height + 1`, and could silently disagree the moment a border was added. `geometry`
//! itself never touches a `Frame` and never takes `&mut App` — it is safe to call as often as
//! any of its three readers like, always with the same answer for the same inputs.

use ratatui::layout::{Constraint, Direction, Layout, Margin, Position, Rect};
use rune_core::assert_invariant;

use crate::app::App;
use crate::focus::LayoutMode;
use crate::layout_column;
use crate::messages;
use crate::region::Region;
use crate::split::{PaneLimits, Split};

/// Left-pane geometry: the DEFAULT column width, and the smallest the left
/// column and the editor pane may each shrink to before one of them gives
/// way. These feed `LEFT_LIMITS`/`CENTER_LIMITS` below, which `Split::allot`
/// reads instead of any bespoke width query, so there is exactly one place
/// that knows the numbers.
pub const DEFAULT_LEFT_PANE_W: u16 = 22;
pub const MIN_LEFT_PANE_W: u16 = 16;
pub const MIN_CENTER_W: u16 = 24;
/// The fuzzy file finder overlay's own minimum usable width — roughly 3-4
/// path elements at ~12-15 cells each. `resolve` below clamps to this floor
/// (never below it, `MIN_LEFT_PANE_W` permitting) whenever `App::filesearch`
/// is open, overriding whatever the user last dragged the column to; the
/// override never writes `app.splits`, so the column snaps back to its
/// prior width the moment the finder closes.
pub const FILESEARCH_MIN_W: u16 = 48;

pub const DIFF_MIN_PANE_W: u16 = 40;
pub const DIFF_LIMITS: PaneLimits = PaneLimits {
    min: DIFF_MIN_PANE_W,
    collapsible: false,
};

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
    /// The side-by-side diff view's left-pane width; the editable right
    /// pane takes what is left after the one-column divider. Always shown —
    /// the diff pane's own presence is decided by `diff_left_rect`, never
    /// by this `Split`'s `shown` flag.
    pub diff: Split,
}

impl Default for Splits {
    fn default() -> Splits {
        Splits {
            left: Split::new(LEFT_LIMITS, false),
            explorer: Split::new(EXPLORER_LIMITS, true),
            diff: Split::new(DIFF_LIMITS, true),
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
    pub messages: Option<Rect>,
    pub left_block: Option<Rect>,
    pub explorer_inner: Rect,
    pub tabs_divider: Option<Rect>,
    pub tabs_inner: Rect,
    pub center: Rect,
    /// Whether the center pane got a `Block::bordered()` this frame
    /// (`center.width >= 3 && center.height >= 3`).
    pub center_bordered: bool,
    pub title: Option<Rect>,
    /// The in-file search bar's one row, directly below `title` and above
    /// `editor` — `Some` only while `App::search` is open AND the center
    /// pane has room to spare it (a content area one row tall keeps that
    /// row for the title instead; the bar simply doesn't fit that frame).
    pub search_bar: Option<Rect>,
    pub diff_left: Option<Rect>,
    pub editor: Rect,
    /// The area left after the footer and the messages pane (if open) are
    /// carved out — the height the left column is allotted from.
    pub main: Rect,
    /// The two-cell-wide band the user grabs to resize the left column: the
    /// column block's right border plus the centre block's left border.
    /// `None` when the column isn't showing. The VERTICAL grab band needs
    /// no field of its own — it is exactly `tabs_divider`.
    pub left_splitter: Option<Rect>,
    /// The two-cell-wide band the user grabs to resize the diff view's
    /// panes, straddling the one-column divider `draw_diff_left` paints at
    /// `diff_left`'s right edge. `None` whenever `diff_left` is — the diff
    /// view is inactive, or folded to full width.
    pub diff_splitter: Option<Rect>,
    /// The command palette overlay's own floating rect — `None` unless
    /// `App::palette` is open.
    pub palette: Option<Rect>,
}

impl Geometry {
    pub fn pane_at(&self, column: u16, row: u16) -> Option<crate::pane::Pane> {
        let point = Position::new(column, row);
        if self.messages.is_some_and(|rect| rect.contains(point)) {
            return Some(crate::pane::Pane::Messages);
        }
        if self.explorer_inner.contains(point) {
            return Some(crate::pane::Pane::Explorer);
        }
        if self.tabs_inner.contains(point) {
            return Some(crate::pane::Pane::Tabs);
        }
        if self.editor.contains(point) {
            return Some(crate::pane::Pane::Editor);
        }
        if self.diff_left.is_some_and(|rect| rect.contains(point)) {
            return Some(crate::pane::Pane::Editor);
        }
        None
    }
}

/// What `resolve` below decides, before `geometry` carves a single further
/// rect from it: the footer/messages-pane split, and — the one place any of this
/// module ever decides whether the left column and its two sections are
/// painted at all — the left column's single bordered block (`None` when
/// nothing fits) and the Explorer/divider/Tabs split of its inner rect.
/// `mode` is decided AT THE SAME TIME as the rects themselves (each branch
/// below names its own `LayoutMode`), never re-derived from them afterwards
/// — the two could otherwise silently disagree about a case like
/// `ExplorerOnly`, where `left_block` is `Some` but there is no `center` to
/// tell that apart from an ordinary `Split`.
struct Resolved {
    footer: Rect,
    messages: Option<Rect>,
    main_area: Rect,
    left_block: Option<Rect>,
    explorer_inner: Rect,
    tabs_divider: Option<Rect>,
    tabs_inner: Rect,
    center: Rect,
    mode: LayoutMode,
}

/// The ONE function in this module that decides what's painted this frame:
/// `LayoutMode::resolve` and `geometry` below both read it, so they can
/// never silently disagree about visibility. Pure — `&App` only, no
/// `Frame`, no `&mut`. Every subtraction saturates — never panics, however
/// small `area` is (the fuzzer drives `Resize` down to a 1-column, 2-row
/// terminal, and the render tests draw at `(0, 0)` and `(1, 1)`).
fn resolve(area: Rect, app: &App) -> Resolved {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    let main_area = chunks.first().copied().unwrap_or(area);
    let footer = chunks
        .get(1)
        .copied()
        .unwrap_or_else(|| Rect::new(area.x, area.y, area.width, 0));

    let (main_area, messages_area) = if messages::is_open(app) {
        let messages_h = messages::height(app, area.height);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(messages_h)])
            .split(main_area);
        (
            rows.first().copied().unwrap_or(main_area),
            rows.get(1).copied(),
        )
    } else {
        (main_area, None)
    };

    // A frame too narrow to fit BOTH the column's floor and the center's
    // floor side by side must still show the column if the user asked for
    // it — flipping to a full-width `ExplorerOnly` rather than silently
    // dropping to `EditorOnly` the way `Split::allot`'s generic "the
    // non-collapsible trail always wins" rule would (CENTER_LIMITS is
    // deliberately non-collapsible for the ORDINARY side-by-side case, so
    // that rule stays correct there; this is the one place its outcome is
    // overridden when the whole column — not just a sliver of it — is what
    // won't fit).
    let split_fits = main_area.width >= MIN_LEFT_PANE_W.saturating_add(MIN_CENTER_W);

    let layout_column::ColumnResolution {
        left_block,
        explorer_inner,
        tabs_divider,
        tabs_inner,
        center,
        mode,
    } = layout_column::resolve_column(main_area, split_fits, app);

    Resolved {
        footer,
        messages: messages_area,
        main_area,
        left_block,
        explorer_inner,
        tabs_divider,
        tabs_inner,
        center,
        mode,
    }
}

/// This frame's resolved `LayoutMode` — the seam `App::layout_mode` calls
/// into during `update`, never from `draw`. Reads the same `resolve`
/// `geometry` itself draws from, so a focus decision and the frame actually
/// painted can never disagree.
pub fn resolve_mode(area: Rect, app: &App) -> LayoutMode {
    resolve(area, app).mode
}

/// Every rect `render::draw` blits into (see `Geometry`'s own doc) — carved
/// from whatever `resolve` above already decided is painted. Nothing past
/// this point decides shown-vs-hidden for the left column or its two
/// sections; it only does further pure rect arithmetic (the center pane's
/// own border/title/editor split, the splitter grab band) against rects
/// `resolve` already produced.
pub fn geometry(area: Rect, app: &App) -> Geometry {
    let Resolved {
        footer,
        messages: messages_area,
        main_area,
        left_block,
        explorer_inner,
        tabs_divider,
        tabs_inner,
        center,
        mode: _,
    } = resolve(area, app);

    // Derived from `left_block`, never from a raw horizontal-allot result:
    // the two can disagree in the both-sections-collapsed case `resolve`
    // handles above, where the horizontal allot succeeds but the column
    // still shows nothing.
    let left_splitter = left_block.map(|b| {
        let x = b.x.saturating_add(b.width).saturating_sub(1);
        crate::region::Region::band_within(main_area, x, 2).rect()
    });

    // The center pane gets a `Block::bordered()` whenever
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

    // There is no breadcrumb rect: the breadcrumb is spliced
    // directly onto the bordered block's own bottom border row
    // (`breadcrumb::overlay`, reading `geo.center` + `geo.center_bordered`
    // itself) rather than reserving a second content row the way an
    // earlier `center_chrome_rows` did.
    let title = (content.height >= 1).then(|| Region::carve_top(content, 1).rect());

    // One extra row for the in-file search bar, below the
    // title and above the editor text, only while `App::search` is open
    // AND the content area actually has a second row to spare it — a
    // one-row-tall content area keeps that single row for the title
    // instead, same "drop the chrome before the essential row" shape the
    // center border itself already uses.
    let search_bar = (app.search().is_some() && content.height >= 2)
        .then(|| Region::row(content, content.y.saturating_add(1), 1).rect());
    let editor_y = 1u16.saturating_add(u16::from(search_bar.is_some()));
    let editor = Region::carve_bottom(content, content.height.saturating_sub(editor_y)).rect();

    let diff_left = diff_left_rect(editor, app);
    let diff_splitter = diff_left.map(|left| {
        let x = left.right().saturating_sub(1);
        crate::region::Region::band_within(editor, x, 2).rect()
    });
    let editor = match diff_left {
        Some(left) => {
            let x = left.right().saturating_add(1);
            Rect::new(x, editor.y, editor.right().saturating_sub(x), editor.height)
        }
        None => editor,
    };

    // The defect class this guards against: "a frame column nobody owns" —
    // a gap between a pane's rect and its neighbour that no `Block` border
    // and no content actually paints, invisible to any containment/overlap
    // check because nothing OVERLAPS a hole, it's just missing. Every
    // comparison goes through `Rect::right()`/`saturating_add` rather than
    // bare subtraction so a degenerate frame (the fuzzer drives `Resize`
    // down to 1 column, 2 rows) hits a saturated equality instead of an
    // underflow panic.
    assert_invariant!(footer.right() == area.right(), || {
        format!(
            "footer {footer:?} does not reach the frame's right edge {}",
            area.right()
        )
    });
    assert_invariant!(center.right() == area.right(), || {
        format!(
            "center {center:?} does not reach the frame's right edge {}",
            area.right()
        )
    });
    let editor_left_bound = diff_left.map_or(center.x.saturating_add(1), |left| {
        left.right().saturating_add(1)
    });
    if center_bordered {
        assert_invariant!(
            editor.right().saturating_add(1) == center.right() && editor.x == editor_left_bound,
            || {
                format!(
                    "editor {editor:?} is not inset exactly one column inside bordered center {center:?}"
                )
            },
        );
    } else {
        assert_invariant!(editor.right() == center.right(), || {
            format!("editor {editor:?} does not reach unbordered center {center:?}'s right edge")
        });
    }

    let palette = crate::palette::geometry_rect(area, app);

    Geometry {
        footer,
        messages: messages_area,
        left_block,
        explorer_inner,
        tabs_divider,
        tabs_inner,
        center,
        center_bordered,
        title,
        search_bar,
        diff_left,
        editor,
        main: main_area,
        left_splitter,
        diff_splitter,
        palette,
    }
}

fn diff_left_rect(editor: Rect, app: &App) -> Option<Rect> {
    let diff = app.diff.as_ref()?;
    if diff.right != app.active {
        return None;
    }
    if editor.width < DIFF_MIN_PANE_W.saturating_mul(2).saturating_add(1) {
        return None;
    }
    let available = editor.width.saturating_sub(1);
    let fallback = available / 2;
    let (left_w, _) = app.splits.diff.allot(available, fallback, DIFF_LIMITS);
    Some(Rect::new(
        editor.x,
        editor.y,
        left_w.unwrap_or(fallback),
        editor.height,
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "layout_tests.rs"]
mod tests;
