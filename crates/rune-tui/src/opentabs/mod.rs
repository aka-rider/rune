//! The Open Tabs pane: its own `listnav::List` cursor/scroll, and its key
//! handling (`Pane::Tabs`-focused, dispatched from `app::handle_key`'s
//! stage 3). The tab display order and MRU activation order live on
//! `DocumentMap`. Rendering lives in the `render` module.

use crate::app::App;
use crate::keymap::{Binding, KeyCode, KeyInput, KeyOutcome, KeyPattern, Mods, resolve_in};
use crate::listnav;
use crate::pane::Pane;
use crate::runtime::Effects;
use crate::workspace;

pub mod limit;
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

/// The Tabs pane's own commands (plan WP5.S1), resolved via `TABS_BINDINGS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabsCommand {
    Up,
    Down,
    Select,
    Leave,
}

/// Arrow keys move the cursor; Enter opens the selected tab
/// (`workspace::switch_to`, plan WP5.S2 — Select is the ONLY way to switch
/// tabs from a cursor row; jumping straight to a tab by digit, and closing
/// the active document, both now resolve at the global pipeline stage
/// (`^1`-`^0`, `^w` — `keymap::GLOBAL_BINDINGS`) so they work from any pane,
/// not just this one.
pub const TABS_BINDINGS: &[Binding<TabsCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Up, Mods::NONE),
        cmd: TabsCommand::Up,
        help: "up",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Down, Mods::NONE),
        cmd: TabsCommand::Down,
        help: "down",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, Mods::NONE),
        cmd: TabsCommand::Select,
        help: "open",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: TabsCommand::Leave,
        help: "back to editor",
        alias: false,
    },
];

/// Stage 3 of the four-stage key pipeline (plan Context, decision 8) when
/// `app.focus() == Pane::Tabs`. `Select` now needs `Effects` (WP2.S8):
/// blurs the title BEFORE the switch — `switch_to` reassigns `app.active`,
/// and `rename::begin` resolves its subject from the live `app.active`, so
/// blurring after would rename the tab just switched TO, not the one being
/// renamed — then lands focus on the Editor unconditionally, since the
/// target tab already exists (nothing here can fail). Closing the active
/// document resolves at the global pipeline stage (`GlobalCommand::
/// CloseFile`) instead of here, so it works from any pane, not just this
/// one — a dirty tab's eventual save-then-close I/O is triggered later,
/// from the Guard's OWN stage-1 key handling (`guard::handle_guard_key`),
/// which already carries its own `Effects`.
pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    let Some(cmd) = resolve_in(TABS_BINDINGS, key) else {
        return KeyOutcome::Ignored;
    };
    match cmd {
        TabsCommand::Up => move_selection(app, -1),
        TabsCommand::Down => move_selection(app, 1),
        TabsCommand::Select => {
            app.blur_title(effects);
            workspace::switch_to_index(app, app.tabs.nav.cursor);
            app.set_focus_pane(Pane::Editor, effects);
        }
        TabsCommand::Leave => app.set_focus_pane(Pane::Editor, effects),
    }
    KeyOutcome::Consumed
}

fn move_selection(app: &mut App, delta: isize) {
    let len = app.documents.order().len();
    app.tabs.nav.move_by(delta, len);
    ensure_visible(app);
}

/// Scrolls the Tabs pane's window to keep the cursor visible — same
/// follow-margin convention as `explorer::ensure_visible`.
fn ensure_visible(app: &mut App) {
    let len = app.documents.order().len();
    let height = visible_rows(app);
    let margin = (height / 4).min(4);
    app.tabs.nav.follow(len, height, margin, 0);
}

/// The Tabs pane's visible row count — same derivation as the Explorer's,
/// read straight from `layout::geometry`'s `tabs_inner`, without the `-1`:
/// the `Open` divider is its own rect outside `tabs_inner`, and there's no
/// title row here (unlike Explorer's root-path row).
fn visible_rows(app: &App) -> usize {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    (crate::layout::geometry(area, app).tabs_inner.height as usize).max(1)
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

    /// `mru` and `order` must always share membership — just in different
    /// orders — and switching a tab in moves it to the end of `mru`.
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

    /// `close_now` reseats `app.active` at the neighbor WITHOUT going
    /// through `switch_to` — this pins that the reseated neighbor still
    /// lands at the end of `mru`.
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

    /// Discarding a live Explorer preview bypasses `close_now` entirely
    /// (`explorer_preview::remove_preview_document`) — this pins that it
    /// still keeps `mru` in lockstep with `order`.
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
