use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::pane::Pane;
use crate::width::{display_width, truncate_to_width};

/// The one-row `Open` divider that introduces the tab rows inside the left
/// column's single bordered block — the Tabs pane has no border of its own,
/// so this row is also where its FOCUS is signalled (the active-border
/// color while `Pane::Tabs` holds focus, the subtle divider style
/// otherwise), alongside the cursor prefix `draw` puts on the rows below.
///
/// The `─` fill is measured in DISPLAY COLUMNS through the shared width
/// chokepoint, never in bytes, and the label is truncated rather
/// than allowed to overflow when the column is narrower than the label.
pub fn draw_divider(app: &App, area: Rect, frame: &mut Frame) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let style = if app.focus() == Pane::Tabs {
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
/// shortcut (matching the `^1`-`^0` chords `GLOBAL_BINDINGS` binds to jump
/// straight to that tab from any pane), a dirty marker, a diverged-disk
/// marker (`⇄` while THAT document's last known sync classification is
/// `DiskAhead`/`Diverged` — per-doc, since a background tab can diverge
/// while another is active), and the document's display name (`tab_active`
/// for `app.active`, `tab_normal` otherwise). The three markers are
/// fixed-width one-cell columns (blank when off), so a state flip never
/// shifts the row's alignment.
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
    let show_cursor = app.focus() == Pane::Tabs;
    let mut cursor_row: Option<u16> = None;
    let mut active_row: Option<u16> = None;

    for (i, &id) in visible.iter().enumerate() {
        let idx = start + i;
        let Some(doc) = app.doc(id) else { continue };
        let selected = idx == app.tabs.nav.cursor;
        let row_y = lines.len() as u16;
        if selected {
            cursor_row = Some(row_y);
        }
        if id == app.active {
            active_row = Some(row_y);
        }
        let shortcut = (idx + 1) % 10;

        let prefix = if show_cursor && selected {
            "\u{203a} "
        } else {
            "  "
        };
        let dirty_marker = if doc.is_dirty() { "x" } else { " " };
        let pin_marker = if doc.pinned { "*" } else { " " };
        let sync_marker = if doc
            .last_sync
            .is_some_and(rune_db::SyncKind::is_disk_divergent)
        {
            "\u{21c4}"
        } else {
            " "
        };
        // A not-yet-promoted preview renders dimmer than an ordinary tab —
        // active or not, since it can be the active document while the
        // Explorer cursor is still just passing over it — so a glance at
        // the Tabs pane can tell a transient preview apart from a tab the
        // user actually opened.
        let name_style = if doc.is_preview() {
            Style::default()
                .fg(app.theme.chrome.subtle)
                .add_modifier(ratatui::style::Modifier::ITALIC)
        } else if id == app.active {
            app.theme.chrome.tab_active
        } else {
            app.theme.chrome.tab_normal
        };

        lines.push(Line::from(vec![
            Span::raw(prefix),
            Span::styled(format!("{shortcut}:"), app.theme.chrome.tabs_divider),
            Span::styled(pin_marker, app.theme.chrome.tab_pinned),
            Span::styled(dirty_marker, app.theme.chrome.tab_dirty),
            Span::styled(sync_marker, app.theme.chrome.error),
            Span::raw(" "),
            Span::styled(doc.file_name().to_string(), name_style),
        ]));
    }

    frame.render_widget(Paragraph::new(lines).style(Style::default()), area);

    // `row_active_bg` first (painted always, answers "which document am I
    // editing"), `row_cursor_bg` second so it wins where the two overlap
    // (answers "where is my cursor", the focused-pane-only question).
    if let Some(y) = active_row {
        let row = Rect::new(area.x, area.y + y, area.width, 1);
        crate::render::rowbg::fill_row(frame, row, app.theme.chrome.row_active_bg);
    }
    if show_cursor && let Some(y) = cursor_row {
        let row = Rect::new(area.x, area.y + y, area.width, 1);
        crate::render::rowbg::fill_row(frame, row, app.theme.chrome.row_cursor_bg);
    }
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
}
