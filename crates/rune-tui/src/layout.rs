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
use crate::focus::LayoutMode;
use crate::messages;
use crate::split::{PaneLimits, Split};

/// Mirrors `rune-md`'s and `rune-syntax`'s own identically-named
/// `STRICT_INVARIANTS`/`assert_invariant` chokepoint: `true` only in test
/// builds or when this crate's own `strict-invariants` feature is
/// explicitly enabled. Kept as a local copy rather than a shared helper —
/// each crate's gate governs only its own producer-bug invariants.
const STRICT_INVARIANTS: bool = cfg!(any(test, feature = "strict-invariants"));

/// The chokepoint every "this should never happen, but let's be sure"
/// geometry check in this module routes through — CONSTITUTION §1.3 forbids
/// `panic!`/`assert!`/`unwrap` in production code paths, so an ordinary
/// build (including a plain `cargo run`) must degrade gracefully on a
/// geometry-invariant violation rather than take down the user's session;
/// only a test run or an explicit opt-in feature treats it as fatal.
fn assert_invariant(cond: bool, msg: impl FnOnce() -> String) {
    if STRICT_INVARIANTS {
        assert!(cond, "{}", msg());
    }
}

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
    pub messages: Option<Rect>,
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
    /// The area left after the footer and the messages pane (if open) are
    /// carved out — the height the left column is allotted from.
    pub main: Rect,
    /// The two-cell-wide band the user grabs to resize the left column: the
    /// column block's right border plus the centre block's left border.
    /// `None` when the column isn't showing. The VERTICAL grab band needs
    /// no field of its own — it is exactly `tabs_divider`.
    pub left_splitter: Option<Rect>,
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

/// Splits one already-sized left-column block into its Explorer/divider/Tabs
/// sections — shared by the ordinary `Split` column and the narrow-frame
/// full-width `ExplorerOnly` column below, so the two can never diverge on
/// how a block of a given size divides. The trailing `bool` is `false` only
/// when NEITHER section fits even alone (`Split::allot`'s `(None, None)`
/// arm) — the caller's cue to give up on the column entirely rather than
/// show a border around nothing.
fn carve_column(left_area: Rect, app: &App) -> (Rect, Option<Rect>, Rect, bool) {
    let zero = Rect::new(0, 0, 0, 0);
    let inner = left_area.inner(Margin::new(1, 1));
    let budget = explorer_budget(left_area);
    let (explorer_h, tabs_h) =
        app.splits
            .explorer
            .allot(budget, explorer_fallback(left_area), TABS_LIMITS);

    match (explorer_h, tabs_h) {
        (Some(explorer_h), Some(tabs_h)) => {
            let explorer_inner = Rect::new(inner.x, inner.y, inner.width, explorer_h);
            let divider = Rect::new(inner.x, inner.y.saturating_add(explorer_h), inner.width, 1);
            let tabs_inner = Rect::new(inner.x, divider.y.saturating_add(1), inner.width, tabs_h);
            (explorer_inner, Some(divider), tabs_inner, true)
        }
        (None, Some(_)) => {
            // The Explorer collapsed: the divider still labels the section
            // and sits at the very top of the inner rect, with the tab rows
            // filling everything below it.
            let divider = Rect::new(inner.x, inner.y, inner.width, 1);
            let tabs_inner = Rect::new(
                inner.x,
                inner.y.saturating_add(1),
                inner.width,
                inner.height.saturating_sub(1),
            );
            (zero, Some(divider), tabs_inner, true)
        }
        (Some(_), None) => {
            // The tab rows collapsed: no divider at all, so the row
            // `explorer_budget` reserved for it comes back to the Explorer
            // — it takes the WHOLE inner height, not the `budget`-sized
            // number `allot` returned above (that number is measured
            // against `budget`, one row short of `inner.height`).
            (inner, None, zero, true)
        }
        (None, None) => (zero, None, zero, false),
    }
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
        .unwrap_or(Rect::new(area.x, area.y, area.width, 0));

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

    let zero = Rect::new(0, 0, 0, 0);

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

    let (left_block, explorer_inner, tabs_divider, tabs_inner, center, mode) =
        if app.splits.left.is_shown() && !split_fits {
            let (explorer_inner, tabs_divider, tabs_inner, fits) = carve_column(main_area, app);
            if fits {
                // No `center` at all: the column IS the frame this mode.
                let center = Rect::new(
                    main_area.x.saturating_add(main_area.width),
                    main_area.y,
                    0,
                    main_area.height,
                );
                (
                    Some(main_area),
                    explorer_inner,
                    tabs_divider,
                    tabs_inner,
                    center,
                    LayoutMode::ExplorerOnly,
                )
            } else {
                // The column can't show anything even at full width (a
                // frame too SHORT, not just too narrow) — give up on it
                // exactly like the ordinary Split path's own `(None, None)`
                // arm does.
                (None, zero, None, zero, main_area, LayoutMode::EditorOnly)
            }
        } else {
            let (left_w, _trail) =
                app.splits
                    .left
                    .allot(main_area.width, DEFAULT_LEFT_PANE_W, CENTER_LIMITS);
            match left_w {
                None => (None, zero, None, zero, main_area, LayoutMode::EditorOnly),
                Some(left_w) => {
                    let cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Length(left_w), Constraint::Min(0)])
                        .split(main_area);
                    let left_area = cols.first().copied().unwrap_or(main_area);
                    let center = cols.get(1).copied().unwrap_or(main_area);

                    let (explorer_inner, tabs_divider, tabs_inner, fits) =
                        carve_column(left_area, app);
                    if fits {
                        let mode = LayoutMode::Split {
                            explorer: explorer_inner.height > 0,
                            tabs: tabs_inner.height > 0,
                        };
                        (
                            Some(left_area),
                            explorer_inner,
                            tabs_divider,
                            tabs_inner,
                            center,
                            mode,
                        )
                    } else {
                        // Neither section fits even alone: the column
                        // yields the space entirely rather than showing a
                        // border around nothing, and `center` reclaims the
                        // width the column would otherwise have reserved.
                        (None, zero, None, zero, main_area, LayoutMode::EditorOnly)
                    }
                }
            }
        };

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

    // The defect class this guards against: "a frame column nobody owns" —
    // a gap between a pane's rect and its neighbour that no `Block` border
    // and no content actually paints, invisible to any containment/overlap
    // check because nothing OVERLAPS a hole, it's just missing. Every
    // comparison goes through `Rect::right()`/`saturating_add` rather than
    // bare subtraction so a degenerate frame (the fuzzer drives `Resize`
    // down to 1 column, 2 rows) hits a saturated equality instead of an
    // underflow panic.
    assert_invariant(footer.right() == area.right(), || {
        format!(
            "footer {footer:?} does not reach the frame's right edge {}",
            area.right()
        )
    });
    assert_invariant(center.right() == area.right(), || {
        format!(
            "center {center:?} does not reach the frame's right edge {}",
            area.right()
        )
    });
    if center_bordered {
        assert_invariant(
            editor.right().saturating_add(1) == center.right()
                && editor.x == center.x.saturating_add(1),
            || {
                format!(
                    "editor {editor:?} is not inset exactly one column inside bordered center {center:?}"
                )
            },
        );
    } else {
        assert_invariant(editor.right() == center.right(), || {
            format!("editor {editor:?} does not reach unbordered center {center:?}'s right edge")
        });
    }

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
        editor,
        main: main_area,
        left_splitter,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

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

    fn app_with_left_shown() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.splits.left.show();
        app
    }

    /// A frame too NARROW to fit the left column alongside the center pane
    /// must still show the column the user asked for — flipping to a
    /// full-width `LayoutMode::ExplorerOnly` rather than silently dropping
    /// it to `EditorOnly` the way the pre-flip resolver used to.
    #[test]
    fn a_too_narrow_frame_flips_to_explorer_only_instead_of_dropping_the_column() {
        let app = app_with_left_shown();
        let area = Rect::new(0, 0, MIN_LEFT_PANE_W + MIN_CENTER_W - 1, 30);
        let geo = geometry(area, &app);
        assert!(geo.left_block.is_some());
        assert_eq!(
            geo.left_block,
            Some(Rect::new(area.x, area.y, area.width, area.height - 1))
        );
        assert_eq!(resolve_mode(area, &app), LayoutMode::ExplorerOnly);
    }

    /// The converse: the same too-narrow frame with the column HIDDEN
    /// resolves to a full-width `EditorOnly` — the flip is keyed on whether
    /// the user asked for the column, never applied unconditionally.
    #[test]
    fn a_too_narrow_frame_with_the_column_hidden_stays_editor_only() {
        let app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        assert!(!app.splits.left.is_shown());
        let area = Rect::new(0, 0, MIN_LEFT_PANE_W + MIN_CENTER_W - 1, 30);
        assert!(geometry(area, &app).left_block.is_none());
        assert_eq!(resolve_mode(area, &app), LayoutMode::EditorOnly);
    }

    /// A frame narrow AND short enough that the column can't show anything
    /// even at full width still falls back to `EditorOnly` — the flip only
    /// ever trades a dropped column for a full-width one when there is
    /// something to actually paint there.
    #[test]
    fn a_too_narrow_and_too_short_frame_still_falls_back_to_editor_only() {
        let app = app_with_left_shown();
        let area = Rect::new(0, 0, MIN_LEFT_PANE_W + MIN_CENTER_W - 1, 3);
        assert!(geometry(area, &app).left_block.is_none());
        assert_eq!(resolve_mode(area, &app), LayoutMode::EditorOnly);
    }

    /// The height-driven counterpart: a column with enough WIDTH to show
    /// SIDE BY SIDE with the center pane, but too few ROWS for either the
    /// Explorer or the tab rows to fit (`resolve`'s `(None, None)` arm),
    /// resolves to `EditorOnly` — this frame is wide enough that the
    /// narrow-frame flip above never applies; it fails on height alone.
    #[test]
    fn a_too_short_frame_resolves_to_editor_only_not_a_silently_dropped_column() {
        let app = app_with_left_shown();
        let area = Rect::new(0, 0, 40, 3);
        assert!(geometry(area, &app).left_block.is_none());
        assert_eq!(resolve_mode(area, &app), LayoutMode::EditorOnly);
    }

    /// The ordinary case: a roomy frame resolves to `Split` with both
    /// sections painted, matching `geometry`'s own rects.
    #[test]
    fn a_roomy_frame_resolves_to_split_with_both_sections_shown() {
        let app = app_with_left_shown();
        let area = Rect::new(0, 0, 100, 30);
        let geo = geometry(area, &app);
        assert!(geo.left_block.is_some());
        assert!(geo.explorer_inner.height > 0);
        assert!(geo.tabs_inner.height > 0);
        assert_eq!(
            resolve_mode(area, &app),
            LayoutMode::Split {
                explorer: true,
                tabs: true
            }
        );
    }
}
