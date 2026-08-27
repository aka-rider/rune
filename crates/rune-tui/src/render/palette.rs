use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::App;
use crate::palette::args::ArgRow;
use crate::palette::{PaletteMode, PaletteRow, PaletteState, Tier};
use crate::registry::{self, Availability};
use crate::render::fuzzyspan::{display_spans, with_bg};
use crate::width::display_width;

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let Some(state) = app.palette() else {
        return;
    };
    if area.width < 3 || area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(app.theme.chrome.active_border);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut y = inner.y;
    let bottom = inner.y.saturating_add(inner.height);
    if y < bottom {
        draw_editbox(state, Rect::new(inner.x, y, inner.width, 1), frame, app);
        y = y.saturating_add(1);
    }
    if let Some(reason) = &state.refusal
        && y < bottom
    {
        let line = Line::from(Span::styled(reason.clone(), app.theme.chrome.error));
        frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
        y = y.saturating_add(1);
    }
    let show_separator = state.field.is_empty() && !state.recents.is_empty();
    if show_separator && y < bottom {
        let rule = "\u{2500}".repeat(inner.width as usize);
        let line = Line::from(Span::styled(rule, Style::new().fg(app.theme.chrome.subtle)));
        frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
        y = y.saturating_add(1);
    }
    if y >= bottom {
        return;
    }
    let rows_area = Rect::new(inner.x, y, inner.width, bottom.saturating_sub(y));
    let lines = row_lines(
        app,
        state,
        rows_area.height as usize,
        rows_area.width as usize,
    );
    frame.render_widget(Paragraph::new(lines), rows_area);
}

fn draw_editbox(state: &PaletteState, area: Rect, frame: &mut Frame, app: &App) {
    let draft = state.field.text();
    let mut used = display_width(draft);
    let mut spans = vec![Span::styled(draft.to_string(), app.theme.chrome.title_text)];
    if let Some(ghost) = crate::palette::ghost_text(state) {
        used += display_width(&ghost);
        spans.push(Span::styled(
            ghost,
            Style::new().fg(app.theme.chrome.subtle),
        ));
    }
    if (used.min(area.width as usize) as u16) < area.width {
        spans.push(Span::styled(
            " ",
            app.theme.chrome.title_text.add_modifier(Modifier::REVERSED),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn row_lines(app: &App, state: &PaletteState, rows: usize, width: usize) -> Vec<Line<'static>> {
    match state.mode {
        PaletteMode::Name => command_row_lines(app, state, rows, width),
        PaletteMode::Param { .. } => arg_row_lines(app, state, rows, width),
    }
}

fn command_row_lines(
    app: &App,
    state: &PaletteState,
    rows: usize,
    width: usize,
) -> Vec<Line<'static>> {
    if state.rows.is_empty() {
        return vec![Line::from(Span::styled(
            "no matching commands",
            Style::new().fg(app.theme.chrome.subtle),
        ))];
    }
    let window = state.nav.window(state.rows.len(), rows);
    let start = window.start;
    let visible = state.rows.get(window).unwrap_or(&[]);
    visible
        .iter()
        .enumerate()
        .map(|(i, row)| row_line(app, row, start + i == state.nav.cursor, width))
        .collect()
}

fn arg_row_lines(app: &App, state: &PaletteState, rows: usize, width: usize) -> Vec<Line<'static>> {
    if state.arg_rows.is_empty() {
        return vec![Line::from(Span::styled(
            "no matching argument",
            Style::new().fg(app.theme.chrome.subtle),
        ))];
    }
    let window = state.nav.window(state.arg_rows.len(), rows);
    let start = window.start;
    let visible = state.arg_rows.get(window).unwrap_or(&[]);
    visible
        .iter()
        .enumerate()
        .map(|(i, row)| arg_row_line(app, row, start + i == state.nav.cursor, width))
        .collect()
}

fn arg_row_line(app: &App, row: &ArgRow, selected: bool, width: usize) -> Line<'static> {
    let row_bg = selected.then_some(app.theme.chrome.selection_bg);
    let name_style = with_bg(app.theme.chrome.file_normal, row_bg);
    let dim = with_bg(Style::new().fg(app.theme.chrome.subtle), row_bg);

    let prefix = if selected { "\u{203a} " } else { "  " };
    let mut spans = vec![Span::styled(prefix.to_string(), name_style)];

    let avail = width.saturating_sub(display_width(prefix));
    match row.via_alias {
        None => {
            spans.extend(display_spans(
                &row.label,
                &row.indices,
                name_style,
                name_style,
                avail,
                0,
            ));
        }
        Some(alias) => {
            spans.push(Span::styled(row.label.clone(), name_style));
            spans.push(Span::styled(" (".to_string(), dim));
            spans.extend(display_spans(alias, &row.indices, dim, dim, avail, 0));
            spans.push(Span::styled(")".to_string(), dim));
        }
    }
    if row.current {
        spans.push(Span::styled("  current".to_string(), dim));
    }

    let content_w: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    if width > content_w {
        spans.push(Span::styled(" ".repeat(width - content_w), name_style));
    }
    Line::from(spans)
}

fn row_line(app: &App, row: &PaletteRow, selected: bool, width: usize) -> Line<'static> {
    let row_bg = selected.then_some(app.theme.chrome.selection_bg);
    let unavailable = matches!(row.availability, Availability::Unavailable(_));
    let name_style = if unavailable {
        with_bg(Style::new().fg(app.theme.chrome.subtle), row_bg)
    } else {
        with_bg(app.theme.chrome.file_normal, row_bg)
    };

    let prefix = if selected { "\u{203a} " } else { "  " };
    let mut spans = vec![Span::styled(
        prefix.to_string(),
        with_bg(app.theme.chrome.file_normal, row_bg),
    )];

    let Some(spec) = registry::spec(row.id) else {
        return Line::from(spans);
    };
    let avail = width.saturating_sub(display_width(prefix));
    let dim = with_bg(Style::new().fg(app.theme.chrome.subtle), row_bg);
    let name_indices: &[u32] = if row.tier == Tier::HelpHit {
        &[]
    } else {
        &row.indices
    };
    match row.via_alias {
        None => {
            spans.extend(display_spans(
                spec.name,
                name_indices,
                name_style,
                name_style,
                avail,
                0,
            ));
        }
        Some(alias) => {
            spans.push(Span::styled(spec.name.to_string(), name_style));
            spans.push(Span::styled(" (".to_string(), dim));
            spans.extend(display_spans(alias, name_indices, dim, dim, avail, 0));
            spans.push(Span::styled(")".to_string(), dim));
        }
    }

    if selected
        && unavailable
        && let Availability::Unavailable(reason) = &row.availability
    {
        spans.push(Span::styled(format!("  {reason}"), dim));
    } else if row.tier == Tier::HelpHit {
        spans.push(Span::styled("  ".to_string(), dim));
        let content_w: usize = spans.iter().map(|s| display_width(&s.content)).sum();
        spans.extend(display_spans(
            spec.help,
            &row.indices,
            dim,
            dim,
            width.saturating_sub(content_w),
            0,
        ));
    } else if let Some(chord) = registry::chords(row.id).next() {
        let label = chord.label();
        let content_w: usize = spans.iter().map(|s| display_width(&s.content)).sum();
        let label_w = display_width(&label);
        let pad = width.saturating_sub(content_w).saturating_sub(label_w);
        if pad > 0 || content_w + label_w <= width {
            spans.push(Span::styled(
                " ".repeat(pad.max(1)),
                with_bg(app.theme.chrome.file_normal, row_bg),
            ));
            spans.push(Span::styled(label, dim));
        }
    }

    let content_w: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    if width > content_w {
        spans.push(Span::styled(
            " ".repeat(width - content_w),
            with_bg(app.theme.chrome.file_normal, row_bg),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    use super::*;
    use crate::palette::Tier;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.frame = Some(crate::app::FrameSize::new(100, 30));
        app
    }

    #[test]
    fn draw_is_a_no_op_when_the_palette_is_closed() {
        let app = app();
        assert!(app.palette().is_none());
        let buf = crate::testgrid::draw_with(40, 10, |frame| {
            draw(&app, Rect::new(0, 0, 40, 10), frame);
        });
        for y in 0..10 {
            for x in 0..40 {
                let cell = buf.cell((x, y)).expect("cell in bounds");
                assert_eq!(cell.symbol(), " ");
            }
        }
    }

    #[test]
    fn open_palette_renders_rows_and_bolds_the_needle() {
        let mut app = app();
        let mut effects = crate::runtime::Effects::default();
        crate::palette::open(&mut app, &mut effects);
        if let Some(state) = app.palette_mut() {
            state.field.set_text("sav");
        }
        crate::palette::recompute(&mut app);

        let area = crate::layout::geometry(Rect::new(0, 0, 100, 30), &app)
            .palette
            .expect("palette rect");
        let buf = crate::testgrid::draw_with(100, 30, |frame| {
            draw(&app, area, frame);
        });

        let mut found_bold = false;
        for y in 0..area.height {
            for x in 0..area.width {
                let cell = buf.cell((area.x + x, area.y + y)).expect("cell in bounds");
                if cell.symbol() == "s" && cell.modifier.contains(Modifier::BOLD) {
                    found_bold = true;
                }
            }
        }
        assert!(found_bold, "the matched needle must render bold");
    }

    #[test]
    fn an_unavailable_row_renders_with_the_dim_style() {
        use crate::global::GlobalCommand;
        use crate::registry::{Availability, CommandId};
        use std::borrow::Cow;

        let mut app = app();
        let mut effects = crate::runtime::Effects::default();
        crate::palette::open(&mut app, &mut effects);
        if let Some(state) = app.palette_mut() {
            state.rows = vec![PaletteRow {
                id: CommandId::Global(GlobalCommand::Save),
                via_alias: None,
                indices: Vec::new(),
                availability: Availability::Unavailable(Cow::Borrowed("cannot save")),
                tier: Tier::Unavailable,
            }];
        }

        let area = crate::layout::geometry(Rect::new(0, 0, 100, 30), &app)
            .palette
            .expect("palette rect");
        let buf = crate::testgrid::draw_with(100, 30, |frame| {
            draw(&app, area, frame);
        });

        let subtle = app.theme.chrome.subtle;
        let name = "save";
        let mut name_start = None;
        for y in 0..area.height {
            for x in 0..=area.width.saturating_sub(name.len() as u16) {
                let matches = name.chars().enumerate().all(|(i, ch)| {
                    buf.cell((area.x + x + i as u16, area.y + y))
                        .is_some_and(|cell| cell.symbol() == ch.to_string())
                });
                if matches {
                    name_start = Some((x, y));
                }
            }
        }
        let (x, y) = name_start.expect("the row's name text \"save\" must be present on screen");
        for i in 0..name.len() as u16 {
            let cell = buf
                .cell((area.x + x + i, area.y + y))
                .expect("cell in bounds");
            assert_eq!(
                cell.fg, subtle,
                "the unavailable row's NAME cells must render in the dim style"
            );
        }
    }
}
