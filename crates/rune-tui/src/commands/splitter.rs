use ratatui::layout::Rect;

use crate::app::App;
use crate::layout;
use crate::pointer::{Drag, MouseInput, Splitter};
use crate::runtime::Effects;

fn contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

fn clamp_u16(v: i32) -> u16 {
    v.clamp(0, i32::from(u16::MAX)) as u16
}

/// `tabs_divider` is tested before `left_splitter`: at the one cell where
/// the two bands could ever coincide, it sits inside the column and is the
/// one the user actually sees and means to grab.
pub fn begin(app: &mut App, input: MouseInput) -> bool {
    let area = app.frame_area();
    let geo = layout::geometry(area, app);

    if let (Some(left), Some(splitter)) = (geo.diff_left, geo.diff_splitter)
        && contains(splitter, input.column, input.row)
    {
        let grab_delta = left.width as i32 - (input.column as i32 - left.x as i32 + 1);
        app.pointer.drag = Some(Drag::Splitter {
            which: Splitter::DiffPanes,
            grab_delta,
        });
        return true;
    }

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
/// motion just collapsed the section that had it — otherwise keystrokes
/// keep routing to a pane with no on-screen presence.
pub fn drag(app: &mut App, input: MouseInput, effects: &mut Effects) {
    let Some(Drag::Splitter { which, grab_delta }) = app.pointer.drag else {
        return;
    };
    if matches!(which, Splitter::LeftColumn) && app.filesearch().is_some() {
        return;
    }
    let area = app.frame_area();
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
                .map_or_else(|| geo.main.y.saturating_add(1), |b| b.y.saturating_add(1));
            app.splits
                .explorer
                .request(clamp_u16(input.row as i32 - inner_top as i32 + grab_delta));
        }
        Splitter::DiffPanes => {
            if let Some(left) = geo.diff_left {
                app.splits.diff.request(clamp_u16(
                    input.column as i32 - left.x as i32 + 1 + grab_delta,
                ));
            }
        }
    }

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
        app.frame = Some(crate::app::FrameSize::new(100, 30));
        app.splits.left.show();
        let mut effects = Effects::default();
        app.set_focus_pane(Pane::Explorer, &mut effects);
        app
    }

    #[test]
    fn dragging_the_column_away_and_the_toggle_command_reach_the_same_state() {
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
        handle_global_command(&mut commanded, GlobalCommand::ToggleLeft, &mut effects);

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

    #[test]
    fn left_column_drag_is_ignored_while_filesearch_is_active() {
        let mut app = app_with_explorer_focused();
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        let before = app.splits.left;
        app.pointer.drag = Some(Drag::Splitter {
            which: Splitter::LeftColumn,
            grab_delta: 0,
        });
        drag(
            &mut app,
            MouseInput {
                kind: MouseKind::Drag(MouseButton::Left),
                column: 10,
                row: 0,
                shift: false,
                alt: false,
                ctrl: false,
            },
            &mut effects,
        );

        assert_eq!(app.splits.left.is_shown(), before.is_shown());
        assert_eq!(
            app.splits.left.size_hint(0),
            before.size_hint(0),
            "the drag must not write a new desired width"
        );
    }
}
