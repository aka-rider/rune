//! Shared style tokens for chrome components.

use ratatui::style::{Color, Modifier, Style};

// ── Indexed colour constants ──────────────────────────────────────

pub const SUBTLE: Color = Color::Indexed(241);
pub const HIGHLIGHT: Color = Color::Indexed(111);
pub const SPECIAL: Color = Color::Indexed(153);
pub const ERROR: Color = Color::Indexed(196);
pub const SURFACE: Color = Color::Indexed(236);
pub const CODE_BG: Color = Color::Indexed(235);
pub const FILE_TEXT: Color = Color::Indexed(252);

// ── Style factories ───────────────────────────────────────────────

/// Returns style with fg SPECIAL, bold — pane titles.
pub fn pane_title() -> Style {
    Style::new().fg(SPECIAL).add_modifier(Modifier::BOLD)
}

/// Returns style with fg FILE_TEXT — normal file entries.
pub fn file_normal() -> Style {
    Style::new().fg(FILE_TEXT)
}

/// Returns style with fg HIGHLIGHT, bold — selected file entry.
pub fn file_selected() -> Style {
    Style::new().fg(HIGHLIGHT).add_modifier(Modifier::BOLD)
}

/// Returns style with fg SUBTLE — tab-divider characters.
pub fn tabs_divider() -> Style {
    Style::new().fg(SUBTLE)
}

/// Returns style with fg SUBTLE — inactive tab label.
pub fn tab_normal() -> Style {
    Style::new().fg(SUBTLE)
}

/// Returns style with fg HIGHLIGHT, bold — active tab label.
pub fn tab_active() -> Style {
    Style::new().fg(HIGHLIGHT).add_modifier(Modifier::BOLD)
}

/// Returns style with fg SPECIAL — pinned tab label.
pub fn tab_pinned() -> Style {
    Style::new().fg(SPECIAL)
}

/// Returns style with fg ERROR — dirty/modified tab indicator.
pub fn tab_dirty() -> Style {
    Style::new().fg(ERROR)
}

/// Returns style with bg SURFACE — footer background.
pub fn footer() -> Style {
    Style::new().bg(SURFACE)
}

/// Returns style with fg HIGHLIGHT, bg SURFACE, bold — footer key hint.
pub fn footer_key() -> Style {
    Style::new()
        .fg(HIGHLIGHT)
        .bg(SURFACE)
        .add_modifier(Modifier::BOLD)
}

/// Returns style with fg SUBTLE, bg SURFACE — footer hint text.
pub fn footer_hint() -> Style {
    Style::new().fg(SUBTLE).bg(SURFACE)
}

/// Returns style with fg SPECIAL, bg SURFACE — footer meta info.
pub fn footer_meta() -> Style {
    Style::new().fg(SPECIAL).bg(SURFACE)
}

/// Returns style with fg ERROR, bold — error messages.
pub fn error() -> Style {
    Style::new().fg(ERROR).add_modifier(Modifier::BOLD)
}

/// Returns style with fg HIGHLIGHT — active border.
pub fn active_border() -> Style {
    Style::new().fg(HIGHLIGHT)
}

/// Returns style with fg SUBTLE — inactive border.
pub fn inactive_border() -> Style {
    Style::new().fg(SUBTLE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn pane_title_fg_and_bold() {
        let s = pane_title();
        assert_eq!(s.fg, Some(SPECIAL));
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn footer_key_bg() {
        let s = footer_key();
        assert_eq!(s.bg, Some(SURFACE));
    }

    #[test]
    fn active_border_fg() {
        let s = active_border();
        assert_eq!(s.fg, Some(HIGHLIGHT));
    }
}
