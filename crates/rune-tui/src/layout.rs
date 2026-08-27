use ratatui::layout::{Constraint, Direction, Layout, Margin, Position, Rect};
use rune_core::assert_invariant;

use crate::app::App;
use crate::focus::LayoutMode;
use crate::layout_column;
use crate::messages;
use crate::region::Region;
use crate::split::{PaneLimits, Split};

pub const DEFAULT_LEFT_PANE_W: u16 = 22;
pub const MIN_LEFT_PANE_W: u16 = 16;
pub const MIN_CENTER_W: u16 = 24;
// The override never writes `app.splits`, so the column snaps back to
// whatever the user last dragged it to the moment the finder closes.
pub const FILESEARCH_MIN_W: u16 = 48;

pub const DIFF_MIN_PANE_W: u16 = 40;
pub const DIFF_LIMITS: PaneLimits = PaneLimits {
    min: DIFF_MIN_PANE_W,
    collapsible: false,
};

// Inner rows, not block rows: the Explorer spends one row on a header, so
// its floor leaves a header plus two entries.
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

// These are desired sizes: geometry clamps them against the live frame
// every call and never writes back, so narrowing the terminal and
// widening it again restores exactly what the user dragged to.
#[derive(Clone, Copy, Debug)]
pub struct Splits {
    pub left: Split,
    pub explorer: Split,
    // Always shown — the diff pane's own presence is decided by
    // `diff_left_rect`, never by this `Split`'s `shown` flag.
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

pub fn explorer_budget(block: Rect) -> u16 {
    block.height.saturating_sub(2).saturating_sub(1)
}

pub fn explorer_fallback(block: Rect) -> u16 {
    block.height.saturating_sub(2).div_ceil(2)
}

// `explorer_inner`/`tabs_inner` are `Rect`, not `Option<Rect>`: when the
// left pane isn't shown at all, they're the zero rect, and nothing reads
// them in that state except `visible_rows` callers, which only run while
// the corresponding pane can actually be focused.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub footer: Rect,
    pub messages: Option<Rect>,
    pub left_block: Option<Rect>,
    pub explorer_inner: Rect,
    pub tabs_divider: Option<Rect>,
    pub tabs_inner: Rect,
    pub center: Rect,
    pub center_bordered: bool,
    pub title: Option<Rect>,
    pub search_bar: Option<Rect>,
    pub diff_left: Option<Rect>,
    pub editor: Rect,
    pub main: Rect,
    // The vertical grab band needs no field of its own — it is exactly
    // `tabs_divider`.
    pub left_splitter: Option<Rect>,
    pub diff_splitter: Option<Rect>,
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

// `mode` is decided at the same time as the rects themselves, never
// re-derived from them afterwards — the two could otherwise silently
// disagree about a case like `ExplorerOnly`, where `left_block` is `Some`
// but there is no `center` to tell that apart from an ordinary `Split`.
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

// Every subtraction saturates — never panics, however small `area` is (the
// fuzzer drives `Resize` down to a 1-column, 2-row terminal).
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

    // A frame too narrow to fit both floors side by side must still show
    // the column if the user asked for it, flipping to a full-width
    // `ExplorerOnly` rather than silently dropping to `EditorOnly` the way
    // `Split::allot`'s generic "the non-collapsible trail always wins" rule
    // would.
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

pub fn resolve_mode(area: Rect, app: &App) -> LayoutMode {
    resolve(area, app).mode
}

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

    // Derived from `left_block`, never a raw horizontal-allot result: the
    // two can disagree in the both-sections-collapsed case, where the
    // allot succeeds but the column still shows nothing.
    let left_splitter = left_block.map(|b| {
        let x = b.x.saturating_add(b.width).saturating_sub(1);
        crate::region::Region::band_within(main_area, x, 2).rect()
    });

    let center_bordered = center.width >= 3 && center.height >= 3;
    let content = if center_bordered {
        center.inner(Margin::new(1, 1))
    } else {
        center
    };

    // There is no breadcrumb rect: the breadcrumb is spliced directly onto
    // the bordered block's own bottom border row rather than reserving a
    // second content row.
    let title = (content.height >= 1).then(|| Region::carve_top(content, 1).rect());

    // A one-row-tall content area keeps that single row for the title
    // instead of the search bar.
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

    // Every comparison below goes through `Rect::right()`/`saturating_add`
    // rather than bare subtraction, so a degenerate frame (the fuzzer
    // drives `Resize` down to 1 column, 2 rows) hits a saturated equality
    // instead of an underflow panic.
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
