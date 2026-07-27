//! Shared style tokens for chrome components AND (WP2.S2, critic R2) the
//! markdown token theme `render.rs::style_for` used to hardcode as named-
//! color literals — this module is now the ONE style source for both.

use ratatui::style::{Color, Modifier, Style};

use rune_md::emit::StyleId;

// ── Indexed colour constants ──────────────────────────────────────

pub const SUBTLE: Color = Color::Indexed(241);
pub const HIGHLIGHT: Color = Color::Indexed(111);
pub const SPECIAL: Color = Color::Indexed(153);
pub const ERROR: Color = Color::Indexed(196);
pub const SURFACE: Color = Color::Indexed(236);
pub const CODE_BG: Color = Color::Indexed(235);
pub const FILE_TEXT: Color = Color::Indexed(252);

/// Selection background (Go `Selection`, `styles.go:196`) — used by
/// `render.rs::highlight_selection`, migrated here alongside `markdown`
/// below so the cursor-overlay path also draws from the one style source.
pub const SELECTION_BG: Color = Color::Indexed(239);

// ── Markdown-only indexed colors ───────────────────────────────────
//
// Inline `lipgloss.Color("NN")` literals from Go's `pkg/ui/styles/
// styles.go:104-200` `Default()` that never became named `Palette` tokens
// there either — quoted here 1:1 rather than folded into the six tokens
// above, which are this app's CHROME palette, not its markdown one.
const H1_BG: Color = Color::Indexed(23);
const H1_FG: Color = Color::Indexed(230);
const H3_FG: Color = Color::Indexed(63);
const H4_FG: Color = Color::Indexed(39);
/// Go's H6/blockquote/list-marker/code-comment gray (`styles.go`: `Heading
/// H6`, `MdBlockquote`, `ListMarker`, `CodeComment` all use color 245).
const DIM_TEXT: Color = Color::Indexed(245);
/// Go's `CodeString`/`TaskChecked` green (color 114).
const CODE_STRING: Color = Color::Indexed(114);
/// Go's `TableBorder`/`HorizontalRule`/`TaskUnchecked` dark gray (color
/// 240).
const HR_BORDER: Color = Color::Indexed(240);

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

/// Semantic `StyleId` -> `ratatui::style::Style` — the markdown token theme
/// (WP2.S2, critic R2), ported 1:1 from Go's `pkg/ui/styles/styles.go:104-
/// 200` `Default()` wherever a Go token exists. `render.rs::style_for`
/// keeps only the dispatch-site call — this is the ONE style source for
/// `StyleId`.
pub fn markdown(id: StyleId) -> Style {
    let base = Style::default();
    match id {
        StyleId::Text => base,
        // Go `HeadingH1`: bold, fg 230, bg 23.
        StyleId::H1 => base.fg(H1_FG).bg(H1_BG).add_modifier(Modifier::BOLD),
        // Go `HeadingH2`: bold, fg Highlight.
        StyleId::H2 => base.fg(HIGHLIGHT).add_modifier(Modifier::BOLD),
        // Go `HeadingH3`: bold, fg 63.
        StyleId::H3 => base.fg(H3_FG).add_modifier(Modifier::BOLD),
        // Go `HeadingH4`: fg 39, NOT bold.
        StyleId::H4 => base.fg(H4_FG),
        // Go `HeadingH5`: fg Special, not bold.
        StyleId::H5 => base.fg(SPECIAL),
        // Go `HeadingH6`: fg 245, not bold.
        StyleId::H6 => base.fg(DIM_TEXT),
        StyleId::Bold => base.add_modifier(Modifier::BOLD),
        StyleId::Italic => base.add_modifier(Modifier::ITALIC),
        StyleId::BoldItalic => base.add_modifier(Modifier::BOLD | Modifier::ITALIC),
        StyleId::Strike => base.add_modifier(Modifier::CROSSED_OUT),
        StyleId::BoldStrike => base.add_modifier(Modifier::BOLD | Modifier::CROSSED_OUT),
        StyleId::ItalicStrike => base.add_modifier(Modifier::ITALIC | Modifier::CROSSED_OUT),
        StyleId::BoldItalicStrike => {
            base.add_modifier(Modifier::BOLD | Modifier::ITALIC | Modifier::CROSSED_OUT)
        }
        // Go `InlineCode`: fg Highlight, bg Surface.
        StyleId::Code => base.fg(HIGHLIGHT).bg(SURFACE),
        // Go `CodeBlockBg` (bg CodeBg) + `CodePlain` (fg 252) combined: this
        // crate's single `CodeFence` id covers a whole fenced block's body
        // text (`rune-md`'s `emit_code_fence` pushes every content line at
        // this one id, never a separate "plain" id), so it must carry both
        // Go styles' colors together.
        StyleId::CodeFence => base.fg(FILE_TEXT).bg(CODE_BG),
        // Go has one `Link` style; `WikiLink` has no separate Go token, so
        // it shares Link's (fg Highlight, underlined).
        StyleId::Link | StyleId::WikiLink => base.fg(HIGHLIGHT).add_modifier(Modifier::UNDERLINED),
        // Go `MdBlockquote`: fg 245, italic.
        StyleId::Blockquote => base.fg(DIM_TEXT).add_modifier(Modifier::ITALIC),
        // Go `ListMarker`: fg 245.
        StyleId::ListMarker => base.fg(DIM_TEXT),
        // Go splits `TaskChecked`(114)/`TaskUnchecked`(240) by checkbox
        // state; this crate's single `TaskMarker` id (`list_marker_style`)
        // covers the whole "- [x] " marker span with no checked-state
        // signal to key off — picks Go's checked green as the more legible
        // default (documented simplification, not a lost distinction Go
        // itself could preserve here either).
        StyleId::TaskMarker => base.fg(CODE_STRING),
        // Go `HorizontalRule`: fg 240.
        StyleId::Hr => base.fg(HR_BORDER),
        // No Go equivalent (Go doesn't style frontmatter separately) — kept
        // at the pre-migration choice (a dim, de-emphasized tone).
        StyleId::FrontmatterDim => base.fg(SUBTLE),
        // Go `TableBody`: fg 252. This crate's single `Verbatim` id also
        // covers raw HTML/unknown blocks, which Go doesn't style distinctly
        // either.
        StyleId::Verbatim => base.fg(FILE_TEXT),
    }
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

    #[test]
    fn markdown_h1_matches_go_bg_and_fg() {
        let s = markdown(StyleId::H1);
        assert_eq!(s.fg, Some(H1_FG));
        assert_eq!(s.bg, Some(H1_BG));
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn markdown_h4_is_not_bold_unlike_h1_through_h3() {
        let s = markdown(StyleId::H4);
        assert_eq!(s.fg, Some(H4_FG));
        assert!(!s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn markdown_link_and_wikilink_share_one_style() {
        assert_eq!(markdown(StyleId::Link), markdown(StyleId::WikiLink));
    }
}
