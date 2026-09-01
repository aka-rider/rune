use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;

use crate::app::App;
use crate::pane::Pane;
use crate::theme::Theme;
use crate::title::ext_split;

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let focused = app.focus() == Pane::Title;
    let theme = &app.theme;

    let name: &str = if focused {
        app.title.text()
    } else {
        app.shown_doc().file_name()
    };

    let selection = focused.then(|| {
        let cursor = app.title.field().cursor();
        (
            cursor.selection_start().get(),
            cursor.selection_end().get(),
            cursor.position.get(),
        )
    });

    let mut spans = build_spans(
        name,
        ext_split(name),
        focused && app.title.ext_unlocked(),
        selection,
        theme,
    );

    let previewing = app.showing_preview();
    if !focused && !previewing && app.active_doc().is_dirty() {
        spans.push(Span::styled(" \u{2022}", theme.chrome.error));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn draw_left(name: &str, area: Rect, theme: &Theme, frame: &mut Frame) {
    let spans = build_spans(name, ext_split(name), false, None, theme);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn build_spans(
    name: &str,
    split: usize,
    always_bright: bool,
    selection: Option<(usize, usize, usize)>,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let len = name.len();
    let mut bounds = vec![
        0usize,
        name.floor_char_boundary(split),
        name.floor_char_boundary(len),
    ];
    if let Some((start, end, cursor)) = selection {
        bounds.push(name.floor_char_boundary(start));
        bounds.push(name.floor_char_boundary(end));
        bounds.push(name.floor_char_boundary(cursor));
        bounds.push(name.floor_char_boundary(next_grapheme_end(name, cursor)));
    }
    bounds.sort_unstable();
    bounds.dedup();

    let mut spans = Vec::new();
    for pair in bounds.windows(2) {
        let &[a, b] = pair else { continue };
        let text = name.get(a..b).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let mut style = base_style(a, split, always_bright, theme);
        if let Some((start, end, cursor)) = selection {
            if start != end && a >= start && b <= end {
                style = style.bg(theme.chrome.selection_bg);
            }
            if a == cursor {
                style = style.add_modifier(Modifier::REVERSED);
            }
        }
        spans.push(Span::styled(text.to_string(), style));
    }

    if let Some((_, _, cursor)) = selection
        && cursor == len
    {
        let style =
            base_style(cursor, split, always_bright, theme).add_modifier(Modifier::REVERSED);
        spans.push(Span::styled(" ", style));
    }

    spans
}

fn base_style(at: usize, split: usize, always_bright: bool, theme: &Theme) -> Style {
    if always_bright || at < split {
        theme.chrome.title_text
    } else {
        Style::new().fg(theme.chrome.subtle)
    }
}

fn next_grapheme_end(name: &str, at: usize) -> usize {
    let at = at.min(name.len());
    name.get(at..)
        .and_then(|rest| rest.graphemes(true).next())
        .map_or(name.len(), |g| at + g.len())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::catppuccin_mocha(false)
    }

    fn joined(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn unfocused_spans_reconstruct_the_whole_name() {
        let theme = theme();
        let spans = build_spans("lessrc.md", ext_split("lessrc.md"), false, None, &theme);
        assert_eq!(joined(&spans), "lessrc.md");
    }

    #[test]
    fn the_extension_is_dimmed_when_not_bright() {
        let theme = theme();
        let split = ext_split("lessrc.md");
        let spans = build_spans("lessrc.md", split, false, None, &theme);
        let stem_span = spans.iter().find(|s| s.content == "lessrc").unwrap();
        assert_eq!(stem_span.style, theme.chrome.title_text);
        let ext_span = spans.iter().find(|s| s.content == ".md").unwrap();
        assert_eq!(ext_span.style, Style::new().fg(theme.chrome.subtle));
    }

    #[test]
    fn always_bright_uses_title_text_throughout() {
        let theme = theme();
        let split = ext_split("lessrc.md");
        let spans = build_spans("lessrc.md", split, true, None, &theme);
        for span in &spans {
            assert_eq!(span.style, theme.chrome.title_text);
        }
    }

    #[test]
    fn a_selection_gets_the_selection_background() {
        let theme = theme();
        let name = "lessrc.md";
        let split = ext_split(name);
        let spans = build_spans(name, split, false, Some((0, 4, 4)), &theme);
        let selected = spans.iter().find(|s| s.content == "less").unwrap();
        assert_eq!(selected.style.bg, Some(theme.chrome.selection_bg));
    }

    #[test]
    fn a_cursor_at_end_of_text_appends_one_reversed_space() {
        let theme = theme();
        let name = "lessrc";
        let split = ext_split(name);
        let spans = build_spans(name, split, true, Some((6, 6, 6)), &theme);
        let last = spans.last().unwrap();
        assert_eq!(last.content, " ");
        assert!(last.style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn the_cursor_cell_is_reversed_mid_text() {
        let theme = theme();
        let name = "lessrc.md";
        let split = ext_split(name);
        let spans = build_spans(name, split, false, Some((2, 2, 2)), &theme);
        let cursor_span = spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("a reversed cursor span");
        assert_eq!(cursor_span.content, "s");
    }
}
