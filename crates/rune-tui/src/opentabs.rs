//! The Open Tabs pane (plan WP5.S1): the tab display order (`order`, kept
//! in sync at its own chokepoints — `App::open_document` pushes,
//! `workspace::close_now` removes, `workspace::switch_to` only moves the
//! cursor, never reorders), its own `listnav::List` cursor/scroll, and its
//! key handling (`Pane::Tabs`-focused, dispatched from `app::handle_key`'s
//! stage 3). Row layout lives here too — the `Open` divider row and the tab
//! rows beneath it, both drawn inside the left column's single bordered
//! block, whose border is not this pane's to paint.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::document::DocumentId;
use crate::keymap::{Binding, KeyCode, KeyInput, KeyOutcome, KeyPattern, Mods, resolve_in};
use crate::listnav;
use crate::pane::Pane;
use crate::width::{display_width, truncate_to_width};
use crate::workspace;

/// The Open Tabs pane's own state (plan WP5.S1): the tab DISPLAY order
/// (distinct from `App.documents`' `BTreeMap` iteration order, plan
/// Assumption A2) and its cursor/scroll position. `order` always contains
/// every live `DocumentId` exactly once — the initial document from
/// `App::new` is in it from the start (`OpenTabs::new`), every later
/// `App::open_document` call pushes its new id, and `workspace::close_now`
/// removes a closed one.
pub struct OpenTabs {
    pub order: Vec<DocumentId>,
    pub nav: listnav::List,
}

impl OpenTabs {
    /// Seeds `order` with the ONE document `App::new` always starts with —
    /// mirroring how `App::new` mints that document directly rather than
    /// going through `App::open_document`.
    pub fn new(initial: DocumentId) -> OpenTabs {
        OpenTabs {
            order: vec![initial],
            nav: listnav::List { cursor: 0, top: 0 },
        }
    }
}

/// The Tabs pane's own commands (plan WP5.S1), resolved via `TABS_BINDINGS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabsCommand {
    Up,
    Down,
    Select,
    Close,
}

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

/// Arrow keys move the cursor; Enter opens the selected tab
/// (`workspace::switch_to`, plan WP5.S2 — Select is the ONLY way to switch
/// tabs in this MVP, digit shortcuts are deferred); `^w` closes it — the
/// same chord Go binds for `CloseFile` (`ctrl+w`, help "close").
pub const TABS_BINDINGS: &[Binding<TabsCommand>] = &[
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, Mods::NONE)],
        cmd: TabsCommand::Up,
        help: "up",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, Mods::NONE)],
        cmd: TabsCommand::Down,
        help: "down",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Enter, Mods::NONE)],
        cmd: TabsCommand::Select,
        help: "open",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('w'), CTRL)],
        cmd: TabsCommand::Close,
        help: "close",
        when: "",
    },
];

/// Stage 3 of the four-stage key pipeline (plan Context, decision 8) when
/// `app.focus == Pane::Tabs`. Unlike `explorer::handle_key`, no arm here
/// ever needs `Effects`: `Select`/`Close` are same-tick direct calls
/// (`workspace::switch_to`/`request_close`, decision 10) — a dirty tab's
/// eventual save-then-close I/O is triggered later, from the Guard modal's
/// OWN stage-1 key handling (`banner::handle_key`), which already carries
/// its own `Effects`.
pub fn handle_key(app: &mut App, key: KeyInput) -> KeyOutcome {
    let Some(cmd) = resolve_in(TABS_BINDINGS, key) else {
        return KeyOutcome::Ignored;
    };
    match cmd {
        TabsCommand::Up => move_selection(app, -1),
        TabsCommand::Down => move_selection(app, 1),
        TabsCommand::Select => {
            if let Some(&id) = app.tabs.order.get(app.tabs.nav.cursor) {
                workspace::switch_to(app, id);
            }
        }
        TabsCommand::Close => {
            if let Some(&id) = app.tabs.order.get(app.tabs.nav.cursor) {
                workspace::request_close(app, id);
            }
        }
    }
    KeyOutcome::Consumed
}

fn move_selection(app: &mut App, delta: isize) {
    let len = app.tabs.order.len();
    app.tabs.nav.move_by(delta, len);
    ensure_visible(app);
}

/// Scrolls the Tabs pane's window to keep the cursor visible — same
/// follow-margin convention as `explorer::ensure_visible`.
fn ensure_visible(app: &mut App) {
    let len = app.tabs.order.len();
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

/// The one-row `Open` divider that introduces the tab rows inside the left
/// column's single bordered block — the Tabs pane has no border of its own,
/// so this row is also where its FOCUS is signalled (the active-border
/// color while `Pane::Tabs` holds focus, the subtle divider style
/// otherwise), alongside the cursor prefix `draw` puts on the rows below.
///
/// The `─` fill is measured in DISPLAY COLUMNS through the shared width
/// chokepoint, never in bytes (§1.5), and the label is truncated rather
/// than allowed to overflow when the column is narrower than the label.
pub fn draw_divider(app: &App, area: Rect, frame: &mut Frame) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let style = if app.focus == Pane::Tabs {
        app.theme.chrome.active_border
    } else {
        app.theme.chrome.tabs_divider
    };

    let total = area.width as usize;
    let label = truncate_to_width(" Open ", total);
    let fill = "\u{2500}".repeat(total.saturating_sub(display_width(&label)));

    let row = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(label, style),
            Span::styled(fill, style),
        ])),
        row,
    );
}

/// Draws the Tabs pane's content into `area` — the rows below the `Open`
/// divider, inside the left column's single bordered block: one row
/// per open tab, in `order`: a `>` cursor prefix (shown only while the
/// Tabs pane itself has focus — unlike Explorer's cursor, which is always
/// shown regardless of `app.focus`, this pane's cursor is meaningless
/// until the user has actually tabbed into it), a `(i+1)%10:` digit
/// shortcut (display-only in this MVP — plan WP5.S2 defers the actual
/// `⌘1..⌘0` chords), a dirty marker, and the document's display name
/// (`tab_active` for `app.active`, `tab_normal` otherwise).
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    if area.height == 0 {
        return;
    }
    let mut lines = Vec::with_capacity(area.height as usize);
    let window = app
        .tabs
        .nav
        .window(app.tabs.order.len(), area.height as usize);
    let start = window.start;
    let visible = app.tabs.order.get(window).unwrap_or(&[]);
    let show_cursor = app.focus == Pane::Tabs;

    for (i, &id) in visible.iter().enumerate() {
        let idx = start + i;
        let Some(doc) = app.doc(id) else { continue };
        let selected = idx == app.tabs.nav.cursor;
        let shortcut = (idx + 1) % 10;

        let prefix = if show_cursor && selected {
            "\u{203a} "
        } else {
            "  "
        };
        let dirty_marker = if doc.is_dirty() { "x" } else { " " };
        let name_style = if id == app.active {
            app.theme.chrome.tab_active
        } else {
            app.theme.chrome.tab_normal
        };

        lines.push(Line::from(vec![
            Span::raw(prefix),
            Span::styled(format!("{shortcut}:"), app.theme.chrome.tabs_divider),
            Span::raw(" "),
            Span::styled(dirty_marker, app.theme.chrome.tab_dirty),
            Span::raw(" "),
            Span::styled(doc.file_name().to_string(), name_style),
        ]));
    }

    frame.render_widget(Paragraph::new(lines).style(Style::default()), area);
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

    /// Renders just the divider into a `width`-wide, 1-row terminal and
    /// returns the row's text — through `testgrid::draw_with` (plan
    /// WP13.S5), the crate's one `TestBackend` construction site, rather
    /// than rolling a tenth hand-written copy here.
    fn divider_row(app: &App, width: u16) -> String {
        let buf = crate::testgrid::draw_with(width, 1, |frame| {
            draw_divider(app, Rect::new(0, 0, width, 1), frame)
        });
        (0..width)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()))
            .collect()
    }

    #[test]
    fn the_divider_fills_its_whole_width_after_the_label() {
        let app = app();
        assert_eq!(
            divider_row(&app, 20),
            format!(" Open {}", "\u{2500}".repeat(14))
        );
    }

    /// Narrower than the label: the label is truncated at a grapheme
    /// boundary rather than overflowing the column.
    #[test]
    fn a_narrow_divider_truncates_the_label_instead_of_overflowing() {
        let app = app();
        for width in 1u16..=6 {
            let row = divider_row(&app, width);
            assert_eq!(
                row.chars().count(),
                width as usize,
                "width {width} must render exactly {width} cells: {row:?}"
            );
            assert!(
                " Open ".starts_with(&row) || row.chars().all(|c| c == '\u{2500}' || c == ' '),
                "width {width} must render a prefix of the label: {row:?}"
            );
        }
        assert_eq!(divider_row(&app, 6), " Open ");
    }

    #[test]
    fn a_zero_width_divider_draws_nothing() {
        let app = app();
        assert_eq!(divider_row(&app, 0), "");
    }

    #[test]
    fn open_document_pushes_onto_tabs_order() {
        let mut app = app();
        let initial = app.active;
        assert_eq!(app.tabs.order, vec![initial]);

        let second = app.open_document(Buffer::new("second"));
        assert_eq!(app.tabs.order, vec![initial, second]);
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
        assert_eq!(handle_key(&mut app, up), KeyOutcome::Consumed);
        assert_eq!(app.tabs.nav.cursor, 0, "clamped at the top");

        let down = KeyInput {
            code: KeyCode::Down,
            mods: Mods::NONE,
        };
        for _ in 0..10 {
            assert_eq!(handle_key(&mut app, down), KeyOutcome::Consumed);
        }
        assert_eq!(app.tabs.nav.cursor, 2, "clamped at the bottom");
    }

    #[test]
    fn select_switches_to_the_cursor_tab() {
        let mut app = app();
        let second = app.open_document(Buffer::new("b"));
        app.tabs.nav.cursor = 1;

        let outcome = handle_key(
            &mut app,
            KeyInput {
                code: KeyCode::Enter,
                mods: Mods::NONE,
            },
        );

        assert_eq!(outcome, KeyOutcome::Consumed);
        assert_eq!(app.active, second);
        assert_eq!(app.focus, Pane::Editor);
    }
}
