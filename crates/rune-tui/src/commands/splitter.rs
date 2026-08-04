//! Splitter drag gestures: hit-testing the two grab bands `layout::geometry`
//! exposes (the left column's own border band, and the `Open` divider row
//! already drawn between the Explorer and the tab rows) and moving the
//! corresponding `Split` while a drag is latched. Every offset here is
//! total — a pointer that has wandered off every rect, or a terminal too
//! small to show either band, must clamp rather than panic or truncate.

use ratatui::layout::Rect;

use crate::app::App;
use crate::layout;
use crate::pointer::{Drag, MouseInput, Splitter};
use crate::runtime::Effects;

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

/// Moves whichever splitter the latched drag is grabbing, then routes
/// through `focus::reconcile` so focus hands back to the Editor if the
/// motion just collapsed the section that had it. Without that handoff,
/// keystrokes keep routing to a pane with no on-screen presence: both
/// `visible_rows` helpers `.max(1)`, so they report a visible row for a
/// zero-height rect and hide the problem.
pub fn drag(app: &mut App, input: MouseInput, effects: &mut Effects) {
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

    // Same reconciliation the command path runs after `CollapseLeft`
    // (`pane::handle_global_command`) — a drag that just collapsed the
    // section holding focus reaches exactly the state a keybinding
    // collapsing it would, through the one shared chokepoint rather than a
    // second, independently-maintained copy of the check.
    crate::focus::reconcile(app, effects);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keymap::GlobalCommand;
    use crate::pane::{Pane, handle_global_command};
    use crate::pointer::{MouseButton, MouseKind};
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with_explorer_focused() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.frame_width = 100;
        app.frame_height = 30;
        app.splits.left.show();
        let mut effects = Effects::default();
        app.set_focus_pane(Pane::Explorer, &mut effects);
        app
    }

    /// Plan WP1: dragging the left column's splitter away and pressing
    /// `GlobalCommand::CollapseLeft` must reach IDENTICAL state — both hide
    /// the same `Split` and both redirect focus through the one shared
    /// `focus::reconcile` chokepoint, so a user reaching for either gesture
    /// is never surprised the other one behaves differently.
    #[test]
    fn dragging_the_column_away_and_the_collapse_command_reach_the_same_state() {
        let mut dragged = app_with_explorer_focused();
        let mut effects = Effects::default();
        dragged.pointer.drag = Some(Drag::Splitter {
            which: Splitter::LeftColumn,
            grab_delta: 0,
        });
        drag(
            &mut dragged,
            MouseInput {
                kind: MouseKind::Drag(MouseButton::Left),
                column: 0,
                row: 0,
                shift: false,
                alt: false,
                ctrl: false,
            },
            &mut effects,
        );

        let mut commanded = app_with_explorer_focused();
        let mut effects = Effects::default();
        handle_global_command(&mut commanded, GlobalCommand::CollapseLeft, &mut effects);

        assert!(
            !dragged.splits.left.is_shown(),
            "test setup: the drag must actually collapse the column"
        );
        assert_eq!(
            dragged.splits.left.is_shown(),
            commanded.splits.left.is_shown()
        );
        assert_eq!(dragged.focus(), commanded.focus());
        assert_eq!(dragged.focus(), Pane::Editor);
    }
}
