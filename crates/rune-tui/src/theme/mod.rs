pub mod catppuccin;
pub mod icons;
pub mod probe;
pub mod quantize;

use ratatui::style::{Color, Modifier, Style};

use catppuccin::Mocha;
use quantize::to_ansi256;
use rune_syntax::ScopeId;
use rune_syntax::scope::{CODE_SCOPES, IMAGE_SCOPE_ID, MarkdownScope, scope_table};

#[derive(Clone, Copy, Debug)]
pub struct ChromeStyles {
    pub pane_title: Style,
    pub file_normal: Style,
    pub dir_normal: Style,
    pub link_dir: Style,
    pub link_file: Style,
    pub link_broken: Style,
    pub tabs_divider: Style,
    pub tab_normal: Style,
    pub tab_active: Style,
    pub tab_pinned: Style,
    pub tab_dirty: Style,
    pub footer: Style,
    pub footer_key: Style,
    pub footer_key_inactive: Style,
    pub footer_hint: Style,
    pub footer_meta: Style,
    pub error: Style,
    pub active_border: Style,
    pub inactive_border: Style,
    pub title_text: Style,
    pub warn: Style,
    pub special: Color,
    pub subtle: Color,
    pub selection_bg: Color,
    pub code_bg: Color,
    pub merge_ours_bg: Style,
    pub merge_theirs_bg: Style,
    // A match highlight and a live text selection can both be on screen at
    // once, so this stays its own field rather than reusing `selection_bg`.
    pub search_match_bg: Style,
    pub bracket_match_bg: Style,
    // A left-column cursor row and an editor text selection can both be on
    // screen in the same frame, so this stays its own field rather than
    // reusing `selection_bg`.
    pub row_cursor_bg: Style,
    pub row_active_bg: Style,
    pub diff_word_ours: Style,
    pub diff_word_theirs: Style,
}

#[derive(Clone, Debug)]
pub struct Theme {
    scopes: Vec<Style>,
    pub chrome: ChromeStyles,
}

impl Theme {
    // Quantization and scope-table walking below both happen exactly once,
    // here at construction, never per frame.
    pub fn catppuccin_mocha(quantized: bool) -> Theme {
        let p = Mocha::palette();
        let c = move |rgb: Color| -> Color { if quantized { to_ansi256(rgb) } else { rgb } };

        let chrome = ChromeStyles {
            pane_title: Style::new().fg(c(p.mauve)).add_modifier(Modifier::BOLD),
            file_normal: Style::new().fg(c(p.text)),
            dir_normal: Style::new().fg(c(p.blue)).add_modifier(Modifier::BOLD),
            link_dir: Style::new().fg(c(p.sapphire)).add_modifier(Modifier::BOLD),
            link_file: Style::new().fg(c(p.teal)),
            link_broken: Style::new().fg(c(p.red)),
            tabs_divider: Style::new().fg(c(p.overlay1)),
            tab_normal: Style::new().fg(c(p.overlay1)),
            tab_active: Style::new().fg(c(p.text)).add_modifier(Modifier::BOLD),
            tab_pinned: Style::new().fg(c(p.mauve)),
            tab_dirty: Style::new().fg(c(p.red)),
            footer: Style::new().bg(c(p.surface0)),
            footer_key: Style::new()
                .fg(c(p.blue))
                .bg(c(p.surface0))
                .add_modifier(Modifier::BOLD),
            footer_key_inactive: Style::new().fg(c(p.overlay1)).bg(c(p.surface0)),
            footer_hint: Style::new().fg(c(p.overlay1)).bg(c(p.surface0)),
            footer_meta: Style::new().fg(c(p.mauve)).bg(c(p.surface0)),
            error: Style::new().fg(c(p.red)).add_modifier(Modifier::BOLD),
            active_border: Style::new().fg(c(p.blue)),
            inactive_border: Style::new().fg(c(p.overlay1)),
            title_text: Style::new().fg(c(p.yellow)).add_modifier(Modifier::BOLD),
            warn: Style::new().fg(c(p.yellow)),
            special: c(p.mauve),
            subtle: c(p.overlay1),
            selection_bg: c(p.surface2),
            code_bg: c(p.surface0),
            merge_ours_bg: Style::new().bg(c(blend(p.surface0, p.green, 0.35))),
            merge_theirs_bg: Style::new().bg(c(blend(p.surface0, p.red, 0.35))),
            search_match_bg: Style::new().bg(c(blend(p.surface0, p.peach, 0.55))),
            bracket_match_bg: Style::new().bg(c(blend(p.surface0, p.sky, 0.45))),
            row_cursor_bg: Style::new().bg(c(p.surface2)),
            row_active_bg: Style::new().bg(c(p.surface0)),
            diff_word_ours: Style::new().bg(c(blend(p.surface0, p.green, 0.6))),
            diff_word_theirs: Style::new().bg(c(blend(p.surface0, p.red, 0.6))),
        };

        let table = scope_table();
        let mut scopes = vec![Style::default(); table.len()];
        for scope in MarkdownScope::ALL {
            let id: ScopeId = (*scope).into();
            if let Some(slot) = scopes.get_mut(id.0 as usize) {
                *slot = markdown_scope_style(*scope, &p, &c);
            }
        }
        if let Some(slot) = scopes.get_mut(IMAGE_SCOPE_ID.0 as usize) {
            *slot = Style::default();
        }
        for name in CODE_SCOPES {
            if let Some(id) = table.resolve(name)
                && let Some(slot) = scopes.get_mut(id.0 as usize)
            {
                *slot = code_scope_style(name, &p, &c);
            }
        }

        Theme { scopes, chrome }
    }

    pub fn scope_style(&self, id: ScopeId) -> Style {
        self.scopes.get(id.0 as usize).copied().unwrap_or_default()
    }

    // ratatui's `Style::patch` merges field-wise, so a scope style that
    // carried a `bg` here would silently clobber whatever background the
    // base cell already had (a code region's background rectangle, an
    // inline code span's own chip) instead of layering over it.
    pub fn overlay_scope_style(&self, id: ScopeId) -> Style {
        Style {
            bg: None,
            ..self.scope_style(id)
        }
    }
}

fn blend(a: Color, b: Color, t: f32) -> Color {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return a;
    };
    let mix =
        |x: u8, y: u8| -> u8 { (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8 };
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

fn markdown_scope_style(scope: MarkdownScope, p: &Mocha, c: &impl Fn(Color) -> Color) -> Style {
    let base = Style::default();
    match scope {
        MarkdownScope::Text => base.fg(c(p.text)),
        MarkdownScope::Heading1 => base.fg(c(p.red)).add_modifier(Modifier::BOLD),
        MarkdownScope::Heading2 => base.fg(c(p.peach)).add_modifier(Modifier::BOLD),
        MarkdownScope::Heading3 => base.fg(c(p.yellow)).add_modifier(Modifier::BOLD),
        MarkdownScope::Heading4 => base.fg(c(p.green)),
        MarkdownScope::Heading5 => base.fg(c(p.teal)),
        MarkdownScope::Heading6 => base.fg(c(p.overlay1)),
        MarkdownScope::Strong => base.add_modifier(Modifier::BOLD),
        MarkdownScope::Italic => base.add_modifier(Modifier::ITALIC),
        MarkdownScope::Strikethrough => base.add_modifier(Modifier::CROSSED_OUT),
        MarkdownScope::RawInline => base.fg(c(p.sapphire)).bg(c(p.surface1)),
        MarkdownScope::RawBlock => base.fg(c(p.text)),
        MarkdownScope::Link => base.fg(c(p.blue)).add_modifier(Modifier::UNDERLINED),
        MarkdownScope::Quote => base.fg(c(p.overlay1)).add_modifier(Modifier::ITALIC),
        MarkdownScope::QuoteMarker => base.fg(c(p.overlay0)),
        MarkdownScope::List => base.fg(c(p.overlay1)),
        MarkdownScope::ListChecked => base.fg(c(p.green)),
        MarkdownScope::TableHeader => base.fg(c(p.text)).add_modifier(Modifier::BOLD),
        MarkdownScope::Table => base.fg(c(p.text)),
        MarkdownScope::TableSeparator => base.fg(c(p.surface2)),
        MarkdownScope::TableBorder => base.fg(c(p.surface2)),
        MarkdownScope::PunctuationSpecial => base.fg(c(p.overlay0)),
        MarkdownScope::Comment => base.fg(c(p.overlay1)),
    }
}

fn code_scope_style(name: &str, p: &Mocha, c: &impl Fn(Color) -> Color) -> Style {
    let base = Style::default();
    match name {
        "keyword" => base.fg(c(p.mauve)),
        "function" | "function.method" => base.fg(c(p.blue)),
        "type" | "type.builtin" => base.fg(c(p.yellow)),
        "constructor" => base.fg(c(p.sapphire)),
        "variable" => base.fg(c(p.text)),
        "variable.parameter" | "variable.member" | "property" => base.fg(c(p.lavender)),
        "constant" | "constant.builtin" => base.fg(c(p.peach)),
        "string" | "string.escape" | "string.regexp" => base.fg(c(p.green)),
        "number" | "boolean" => base.fg(c(p.peach)),
        "operator" => base.fg(c(p.sky)),
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => base.fg(c(p.overlay1)),
        "attribute" => base.fg(c(p.yellow)),
        "label" => base.fg(c(p.sapphire)),
        "tag" => base.fg(c(p.blue)),
        // `name` is a `&str`, not an exhaustive enum, so the compiler can't
        // guarantee this match covers every capture name a grammar may
        // emit; an unmatched one degrades to plain, unstyled text.
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_scope_gets_a_non_default_or_deliberately_plain_style() {
        for quantized in [false, true] {
            let theme = Theme::catppuccin_mocha(quantized);
            let table = scope_table();
            for (id, _name) in table.iter() {
                let _ = theme.scope_style(id);
            }
        }
    }

    #[test]
    fn scope_style_falls_back_to_default_past_the_known_table() {
        let theme = Theme::catppuccin_mocha(false);
        let far = ScopeId(u16::MAX);
        assert_eq!(theme.scope_style(far), Style::default());
    }

    #[test]
    fn quantized_theme_uses_only_indexed_colors() {
        let theme = Theme::catppuccin_mocha(true);
        assert!(matches!(
            theme.chrome.active_border.fg,
            Some(Color::Indexed(_))
        ));
        let table = scope_table();
        for (id, _name) in table.iter() {
            let style = theme.scope_style(id);
            if let Some(fg) = style.fg {
                assert!(matches!(fg, Color::Indexed(_)), "fg {fg:?} not quantized");
            }
            if let Some(bg) = style.bg {
                assert!(matches!(bg, Color::Indexed(_)), "bg {bg:?} not quantized");
            }
        }
    }

    #[test]
    fn overlay_scope_style_never_carries_a_background() {
        for quantized in [false, true] {
            let theme = Theme::catppuccin_mocha(quantized);
            let table = scope_table();
            for (id, name) in table.iter() {
                let style = theme.overlay_scope_style(id);
                assert_eq!(style.bg, None, "scope {name} unexpectedly carries a bg");
            }
        }
    }

    #[test]
    fn code_foreground_never_matches_a_heading() {
        for quantized in [false, true] {
            let theme = Theme::catppuccin_mocha(quantized);
            let table = scope_table();
            let fg_of = |name: &str| table.resolve(name).and_then(|id| theme.scope_style(id).fg);
            let headings: Vec<(String, Option<Color>)> = (1..=6)
                .map(|lvl| {
                    let name = format!("markup.heading.{lvl}");
                    let fg = fg_of(&name);
                    (name, fg)
                })
                .collect();
            for code in ["markup.raw.inline", "markup.raw.block"] {
                let code_fg = fg_of(code);
                assert!(code_fg.is_some(), "{code} has no foreground at all");
                for (heading, heading_fg) in &headings {
                    assert_ne!(
                        code_fg, *heading_fg,
                        "{code} shares its foreground with {heading}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_left_column_row_style_is_visually_distinct() {
        for quantized in [false, true] {
            let chrome = Theme::catppuccin_mocha(quantized).chrome;
            let explorer_rows = [
                ("dir_normal", chrome.dir_normal),
                ("link_dir", chrome.link_dir),
                ("link_file", chrome.link_file),
                ("link_broken", chrome.link_broken),
                ("file_normal", chrome.file_normal),
            ];
            let tab_rows = [
                ("tab_normal", chrome.tab_normal),
                ("tab_active", chrome.tab_active),
                ("tab_pinned", chrome.tab_pinned),
            ];

            for (name, style) in explorer_rows.iter().chain(tab_rows.iter()) {
                assert!(
                    style.fg.is_some(),
                    "{name} has no foreground at all (quantized {quantized})"
                );
            }

            for (i, (name, style)) in explorer_rows.iter().enumerate() {
                for (other_name, other) in explorer_rows.iter().skip(i + 1) {
                    assert_ne!(
                        style.fg, other.fg,
                        "{name} and {other_name} share a hue (quantized {quantized})"
                    );
                }
            }

            let column: Vec<(&str, (Option<Color>, bool))> = explorer_rows
                .iter()
                .chain(tab_rows.iter())
                .map(|(name, style)| {
                    (
                        *name,
                        (style.fg, style.add_modifier.contains(Modifier::BOLD)),
                    )
                })
                .collect();
            for (i, (name, mark)) in column.iter().enumerate() {
                for (other_name, other) in column.iter().skip(i + 1) {
                    assert_ne!(
                        mark, other,
                        "{name} and {other_name} share hue and weight (quantized {quantized})"
                    );
                }
            }
        }
    }

    #[test]
    fn the_cursor_row_background_is_brighter_than_the_active_row_background() {
        let chrome = Theme::catppuccin_mocha(false).chrome;
        let level = |style: Style| match style.bg {
            Some(Color::Rgb(r, g, b)) => u32::from(r) + u32::from(g) + u32::from(b),
            _ => 0,
        };
        assert!(
            level(chrome.row_cursor_bg) > level(chrome.row_active_bg),
            "row_cursor_bg must out-brighten row_active_bg"
        );
    }

    #[test]
    fn code_scopes_never_carry_a_background() {
        let theme = Theme::catppuccin_mocha(false);
        let table = scope_table();
        for name in CODE_SCOPES {
            if let Some(id) = table.resolve(name) {
                let style = theme.scope_style(id);
                assert_eq!(style.bg, None, "scope {name} unexpectedly carries a bg");
            }
        }
    }
}
