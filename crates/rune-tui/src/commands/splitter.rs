//! Splitter drag gestures: hit-testing the two grab bands `layout::geometry`
//! exposes (the left column's own border band, and the `Open` divider row
//! already drawn between the Explorer and the tab rows) and moving the
//! corresponding `Split` while a drag is latched. Every offset here is
//! total — a pointer that has wandered off every rect, or a terminal too
//! small to show either band, must clamp rather than panic or truncate.

use ratatui::layout::Rect;

use crate::app::App;
use crate::layout;
use crate::pane::Pane;
use crate::pointer::{Drag, MouseInput, Splitter};

/// Whether `(col, row)` lands inside `rect`, saturating so a rect pinned at
/// the frame's far edge never wraps.
fn contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

/// Folds a signed cell count back into the `u16` a `Split` stores: below
/// zero clamps to `0`, above `u16::MAX` clamps to `u16::MAX`. The splitter
/// maths above this needs signed arithmetic (a grab delta can be negative),
/// so this is the one place that maps back into the unsigned domain instead
/// of every call site doing its own `as` truncation.
fn clamp_u16(v: i32) -> u16 {
    v.clamp(0, i32::from(u16::MAX)) as u16
}

/// Tests a left mouse-down against the two grab bands, latching
/// `app.pointer.drag` and returning `true` on a hit. `tabs_divider` is
/// tested first — it sits inside the column, so at the one cell where the
/// bands could ever coincide it is the one the user actually sees and
/// means to grab.
pub fn begin(app: &mut App, input: MouseInput) -> bool {
    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    let geo = layout::geometry(area, app);

    // Both grab bands only exist while the column itself is shown, but
    // `Geometry`'s type doesn't tie `left_block` to `tabs_divider`/
    // `left_splitter` — so this guard stays a `let ... else` (never
    // `unwrap`/`expect`, §1.3) even though it's the same check either band
    // needs.
    let Some(left_block) = geo.left_block else {
        return false;
    };

    if let Some(divider) = geo.tabs_divider
        && contains(divider, input.column, input.row)
    {
        let explorer_h = divider.y.saturating_sub(left_block.y.saturating_add(1));
        let grab_delta = explorer_h as i32 - (input.row as i32 - (left_block.y as i32 + 1));
        app.pointer.drag = Some(Drag::Splitter {
            which: Splitter::ExplorerTabs,
            grab_delta,
        });
        return true;
    }

    if let Some(splitter) = geo.left_splitter
        && contains(splitter, input.column, input.row)
    {
        let grab_delta = left_block.width as i32 - (input.column as i32 + 1);
        app.pointer.drag = Some(Drag::Splitter {
            which: Splitter::LeftColumn,
            grab_delta,
        });
        return true;
    }

    false
}

/// Moves whichever splitter the latched drag is grabbing, then hands focus
/// back to the Editor if the motion just collapsed the section that had
/// it. Without that handoff, keystrokes keep routing to a pane with no
/// on-screen presence: both `visible_rows` helpers `.max(1)`, so they
/// report a visible row for a zero-height rect and hide the problem.
pub fn drag(app: &mut App, input: MouseInput) {
    let Some(Drag::Splitter { which, grab_delta }) = app.pointer.drag else {
        return;
    };
    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    let geo = layout::geometry(area, app);

    match which {
        Splitter::LeftColumn => {
            app.splits
                .left
                .request(clamp_u16(input.column as i32 + 1 + grab_delta));
        }
        Splitter::ExplorerTabs => {
            let inner_top = geo
                .left_block
                .map_or(geo.main.y.saturating_add(1), |b| b.y.saturating_add(1));
            app.splits
                .explorer
                .request(clamp_u16(input.row as i32 - inner_top as i32 + grab_delta));
        }
    }

    let geo = layout::geometry(area, app);
    let explorer_just_collapsed = app.focus == Pane::Explorer && geo.explorer_inner.height == 0;
    let tabs_just_collapsed = app.focus == Pane::Tabs && geo.tabs_divider.is_none();
    if explorer_just_collapsed || tabs_just_collapsed {
        app.focus = Pane::Editor;
    }
}
