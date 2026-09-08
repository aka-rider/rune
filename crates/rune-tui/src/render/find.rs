use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use crate::app::App;
use crate::field::TextField;
use crate::find::{ChipKind, Control, FindState, Scope};
use crate::layout_find::{Chip, FindPanelGeometry, chip_chord, chip_text};
use crate::render::queryrow::{QueryRow, build_spans};
use crate::theme::Theme;
use crate::width::{display_width, truncate_to_width};

pub fn draw(app: &App, panel: &FindPanelGeometry, frame: &mut Frame) {
    let Some(state) = app.find() else {
        return;
    };
    let theme = &app.theme;
    let border = if state.focused {
        theme.chrome.active_border
    } else {
        theme.chrome.inactive_border
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(" Find ");
    frame.render_widget(block, panel.frame);

    let readout = readout(app, state, theme);
    let find_focused = state.focused && state.focus == Control::Find;
    let (find_cursor, find_selection) = caret_state(&state.find.editor);
    draw_field(
        frame,
        panel.find_field,
        QueryRow {
            prompt: "",
            text: state.find.editor.text(),
            readout: readout
                .as_ref()
                .map(|(text, style)| (text.as_str(), *style)),
            focused: find_focused,
            cursor: find_cursor,
            selection: find_selection,
        },
        theme,
    );

    if let (Some(replace), Some(field)) = (&state.replace, panel.replace_field) {
        draw_rule(frame, panel.frame, field.y.saturating_sub(1), border);
        let replace_focused = state.focused && state.focus == Control::Replace;
        let (replace_cursor, replace_selection) = caret_state(&replace.editor);
        draw_field(
            frame,
            field,
            QueryRow {
                prompt: "",
                text: replace.editor.text(),
                readout: None,
                focused: replace_focused,
                cursor: replace_cursor,
                selection: replace_selection,
            },
            theme,
        );
    }

    for (chip, rect) in panel.chips.iter().flatten() {
        draw_chip(frame, state, *chip, *rect, theme);
    }
}

fn draw_field(frame: &mut Frame, area: Rect, row: QueryRow<'_>, theme: &Theme) {
    let spans = build_spans(row, area.width as usize, theme);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn caret_state(editor: &TextField) -> (usize, (usize, usize)) {
    let cursor = editor.cursor();
    let position = cursor.position.get();
    let selection = if cursor.has_selection() {
        let (start, end) = cursor.selection_range();
        (start.get(), end.get())
    } else {
        (position, position)
    };
    (position, selection)
}

fn draw_rule(frame: &mut Frame, panel_frame: Rect, y: u16, border: Style) {
    let width = panel_frame.width as usize;
    let label = "\u{251c} Replace ";
    let fill = width.saturating_sub(display_width(label).saturating_add(1));
    let text = format!("{label}{}\u{2524}", "\u{2500}".repeat(fill));
    let line = Line::from(Span::styled(truncate_to_width(&text, width), border));
    frame.render_widget(
        Paragraph::new(line),
        Rect::new(panel_frame.x, y, panel_frame.width, 1),
    );
}

fn draw_chip(frame: &mut Frame, state: &FindState, chip: Chip, rect: Rect, theme: &Theme) {
    let mut spans = vec![
        Span::styled(chip_chord(chip), theme.chrome.footer_key),
        Span::raw(" "),
    ];
    spans.extend(match chip.kind {
        ChipKind::Scope => scope_spans(state.scope(), theme),
        ChipKind::Case | ChipKind::Word | ChipKind::Regex => {
            let on = state.chip_on(chip.kind).unwrap_or(false);
            let style = if on {
                theme.chrome.footer_key
            } else {
                theme.chrome.footer_key_inactive
            };
            vec![Span::styled(chip_text(state, chip), style)]
        }
        ChipKind::ReplaceOne | ChipKind::ReplaceAll => {
            vec![Span::styled(
                chip_text(state, chip),
                theme.chrome.footer_key,
            )]
        }
    });
    frame.render_widget(Paragraph::new(Line::from(spans)), rect);
}

fn scope_spans(scope: Scope, theme: &Theme) -> Vec<Span<'static>> {
    let (file, project) = match scope {
        Scope::File => (theme.chrome.footer_key, theme.chrome.footer_key_inactive),
        Scope::Project => (theme.chrome.footer_key_inactive, theme.chrome.footer_key),
    };
    vec![
        Span::styled("File", file),
        Span::styled("|", theme.chrome.footer_key_inactive),
        Span::styled("Project", project),
    ]
}

fn readout(app: &App, state: &FindState, theme: &Theme) -> Option<(String, Style)> {
    if let Err(error) = &state.pattern {
        let flat: Vec<&str> = error.0.split_whitespace().collect();
        return Some((flat.join(" "), theme.chrome.error));
    }
    if state.project.is_some() {
        return crate::find::project::readout_text(app, state)
            .map(|text| (text, theme.chrome.title_text));
    }
    let text = match (state.current, state.matches.len()) {
        (Some(current), count) => format!("{}/{count}", current.saturating_add(1)),
        (None, count) if count > 0 => count.to_string(),
        (None, _) if !state.find.editor.text().trim().is_empty() => "no matches".to_string(),
        (None, _) => return None,
    };
    Some((text, theme.chrome.title_text))
}
