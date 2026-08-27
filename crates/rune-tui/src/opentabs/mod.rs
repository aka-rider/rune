use ratatui::layout::Rect;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::keymap::{Binding, KeyCode, KeyInput, KeyOutcome, KeyPattern, Mods, resolve_in};
use crate::listnav;
use crate::pane::Pane;
use crate::runtime::Effects;
use crate::workspace;

pub mod limit;
pub(crate) mod mouse;
pub mod render;

pub use render::{draw, draw_divider};

pub struct OpenTabs {
    pub nav: listnav::List,
}

impl OpenTabs {
    pub fn new() -> OpenTabs {
        OpenTabs {
            nav: listnav::List { cursor: 0, top: 0 },
        }
    }
}

impl Default for OpenTabs {
    fn default() -> OpenTabs {
        OpenTabs::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabsCommand {
    Up,
    Down,
    Select,
    Leave,
}

pub const TABS_BINDINGS: &[Binding<TabsCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Up, Mods::NONE),
        cmd: TabsCommand::Up,
        help: "up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Down, Mods::NONE),
        cmd: TabsCommand::Down,
        help: "down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, Mods::NONE),
        cmd: TabsCommand::Select,
        help: "open",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: TabsCommand::Leave,
        help: "back to editor",
        secondary: false,
    },
];

pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    let Some(cmd) = resolve_in(TABS_BINDINGS, key) else {
        return KeyOutcome::Ignored;
    };
    match cmd {
        TabsCommand::Up => move_selection(app, -1),
        TabsCommand::Down => move_selection(app, 1),
        TabsCommand::Select => activate(app, effects),
        TabsCommand::Leave => app.set_focus_pane(Pane::Editor, effects),
    }
    KeyOutcome::Consumed
}

pub(crate) fn activate(app: &mut App, effects: &mut Effects) {
    app.blur_title(effects);
    workspace::select_tab(app, app.tabs.nav.cursor);
    app.set_focus_pane(Pane::Editor, effects);
}

pub(crate) fn select_index(app: &mut App, index: usize) {
    let len = app.documents.order().len();
    app.tabs.nav.cursor = index.min(len.saturating_sub(1));
    ensure_visible(app);
}

fn move_selection(app: &mut App, delta: isize) {
    let len = app.documents.order().len();
    app.tabs.nav.move_by(delta, len);
    select_index(app, app.tabs.nav.cursor);
}

fn ensure_visible(app: &mut App) {
    let len = app.documents.order().len();
    let height = visible_rows(app);
    let margin = (height / 4).min(4);
    app.tabs.nav.follow(len, height, margin, 0);
}

fn visible_rows(app: &App) -> usize {
    let area = app.frame_area();
    entry_rows(crate::layout::geometry(area, app).tabs_inner).max(1)
}

pub(crate) fn entry_rows(rect: Rect) -> usize {
    rect.height as usize
}

pub(crate) fn painted_tabs(app: &App, rect: Rect) -> Vec<(usize, DocumentId, &Document)> {
    let order = app.documents.order();
    let window = app.tabs.nav.window(order.len(), entry_rows(rect));
    let start = window.start;
    order
        .get(window)
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .filter_map(|(offset, &id)| app.doc(id).map(|doc| (start + offset, id, doc)))
        .collect()
}

pub(crate) fn entry_at(app: &App, rect: Rect, row: u16) -> Option<usize> {
    painted_tabs(app, rect)
        .get(row as usize)
        .map(|&(index, _, _)| index)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::app::App;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.active_doc_mut().viewport.set_size(80, 23);
        app
    }

    #[test]
    fn open_document_pushes_onto_tabs_order() {
        let mut app = app();
        let initial = app.active;
        assert_eq!(app.documents.order(), &[initial]);

        let second = app.open_document(Buffer::new("second"));
        assert_eq!(app.documents.order(), &[initial, second]);
    }

    #[test]
    fn up_and_down_clamp_at_the_list_bounds() {
        let mut app = app();
        app.open_document(Buffer::new("b"));
        app.open_document(Buffer::new("c"));

        let up = KeyInput {
            code: KeyCode::Up,
            mods: Mods::NONE,
        };
        let mut effects = Effects::default();
        assert_eq!(handle_key(&mut app, up, &mut effects), KeyOutcome::Consumed);
        assert_eq!(app.tabs.nav.cursor, 0, "clamped at the top");

        let down = KeyInput {
            code: KeyCode::Down,
            mods: Mods::NONE,
        };
        for _ in 0..10 {
            assert_eq!(
                handle_key(&mut app, down, &mut effects),
                KeyOutcome::Consumed
            );
        }
        assert_eq!(app.tabs.nav.cursor, 2, "clamped at the bottom");
    }

    #[test]
    fn mru_tracks_activation_order() {
        let mut app = app();
        let first = app.active;
        app.open_document(Buffer::new("second"));
        app.open_document(Buffer::new("third"));

        crate::workspace::switch_to(&mut app, first);

        assert_eq!(app.documents.mru().last(), Some(&first));

        let mut order_sorted = app.documents.order().to_vec();
        let mut mru_sorted = app.documents.mru().to_vec();
        order_sorted.sort();
        mru_sorted.sort();
        assert_eq!(order_sorted, mru_sorted);
    }

    #[test]
    fn closing_the_active_tab_touches_its_replacement() {
        let mut app = app();
        app.open_document(Buffer::new("second"));
        let target = app.open_document(Buffer::new("third"));
        crate::workspace::switch_to(&mut app, target);

        let mut effects = Effects::default();
        crate::workspace::request_close(&mut app, target, &mut effects);

        assert_ne!(app.active, target);
        assert_eq!(app.documents.mru().last(), Some(&app.active));

        let mut order_sorted = app.documents.order().to_vec();
        let mut mru_sorted = app.documents.mru().to_vec();
        order_sorted.sort();
        mru_sorted.sort();
        assert_eq!(order_sorted, mru_sorted);
    }

    #[test]
    fn preview_discard_keeps_mru_in_lockstep() {
        let mut app = app();
        let target = app.active;
        let preview = app.open_document(Buffer::new("previewed"));
        app.doc_mut(preview).unwrap().read_only = crate::document::ReadOnly::Preview;
        app.explorer.preview = Some(preview);

        crate::workspace::switch_to(&mut app, target);

        assert!(!app.documents.order().contains(&preview));
        assert!(!app.documents.mru().contains(&preview));

        let mut order_sorted = app.documents.order().to_vec();
        let mut mru_sorted = app.documents.mru().to_vec();
        order_sorted.sort();
        mru_sorted.sort();
        assert_eq!(order_sorted, mru_sorted);
    }

    #[test]
    fn select_switches_to_the_cursor_tab() {
        let mut app = app();
        let second = app.open_document(Buffer::new("b"));
        app.tabs.nav.cursor = 1;

        let mut effects = Effects::default();
        let outcome = handle_key(
            &mut app,
            KeyInput {
                code: KeyCode::Enter,
                mods: Mods::NONE,
            },
            &mut effects,
        );

        assert_eq!(outcome, KeyOutcome::Consumed);
        assert_eq!(app.active, second);
        assert_eq!(app.focus(), Pane::Editor);
    }
}
