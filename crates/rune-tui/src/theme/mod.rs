//! The rendered theme (plan WP4 Half 2): the ONE chokepoint every
//! `ratatui::style::Style` in this crate is built from — replaces the
//! pre-WP4 `styles.rs`'s 17 independent chrome-style functions plus its
//! `markdown(id)` funnel with a single `Theme` value held on `App`.
//!
//! `scopes` is indexed by the `ScopeId` `rune-syntax`'s `ScopeTable` hands
//! out — `rune-syntax` owns that table; this module only ever maps a
//! resolved id to a `Style`, never registers or resolves a scope name
//! itself (plan: "rune-syntax owns the table; the theme only maps
//! `ScopeId -> Style`"). `chrome` covers everything that isn't a
//! markdown/code token: pane borders, tab labels, the footer, etc.
//!
//! Colours are stored as truecolor `Color::Rgb` (Catppuccin Mocha,
//! `catppuccin.rs`) and quantized to `Color::Indexed` exactly once, here at
//! construction (`quantize.rs`), never per frame — macOS Terminal.app (the
//! default terminal on the only OS this app supports) is 256-colour only.

pub mod catppuccin;
pub mod probe;
pub mod quantize;

use ratatui::style::{Color, Modifier, Style};

use catppuccin::Mocha;
use quantize::to_ansi256;
use rune_syntax::ScopeId;
use rune_syntax::scope::scope_table;

/// Every chrome (non-markdown/code) style the pre-WP4 `styles.rs` used to
/// build from a raw `Color::Indexed` literal — one field per former
/// function, same name, now a value instead of a call. Plus a few raw
/// colours (`special`/`subtle`/`selection_bg`) that a handful of call
/// sites (`breadcrumb.rs`'s ellipsis/leaf-part spans, `render.rs`'s
/// selection overlay) build their OWN ad hoc `Style` from, rather than
/// using a named chrome style verbatim.
#[derive(Clone, Copy, Debug)]
pub struct ChromeStyles {
    pub pane_title: Style,
    pub file_normal: Style,
    pub file_selected: Style,
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
    pub special: Color,
    pub subtle: Color,
    pub selection_bg: Color,
}

/// The full rendered theme: `scopes` (markdown/code tokens, `ScopeId`
/// indexed) plus `chrome` (everything else). `Theme::catppuccin_mocha` is
/// the only constructor, so quantization (Half 2) and scope-table walking
/// (Half 1) both happen exactly once, at startup — never per frame.
#[derive(Clone, Debug)]
pub struct Theme {
    scopes: Vec<Style>,
    pub chrome: ChromeStyles,
}

impl Theme {
    /// Builds Catppuccin Mocha. `quantized` selects the 256-colour
    /// fallback path (`theme::probe::supports_truecolor` is what a real
    /// terminal decides it from); this constructor itself stays
    /// terminal-free, so it's exercisable in tests without one.
    pub fn catppuccin_mocha(quantized: bool) -> Theme {
        let p = Mocha::palette();
        let c = move |rgb: Color| -> Color { if quantized { to_ansi256(rgb) } else { rgb } };

        let chrome = ChromeStyles {
            pane_title: Style::new().fg(c(p.mauve)).add_modifier(Modifier::BOLD),
            file_normal: Style::new().fg(c(p.text)),
            file_selected: Style::new().fg(c(p.blue)).add_modifier(Modifier::BOLD),
            tabs_divider: Style::new().fg(c(p.overlay1)),
            tab_normal: Style::new().fg(c(p.overlay1)),
            tab_active: Style::new().fg(c(p.blue)).add_modifier(Modifier::BOLD),
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
            special: c(p.mauve),
            subtle: c(p.overlay1),
            selection_bg: c(p.surface2),
        };

        let table = scope_table();
        let mut scopes = vec![Style::default(); table.len()];
        for (id, name) in table.iter() {
            if let Some(slot) = scopes.get_mut(id.0 as usize) {
                *slot = markdown_scope_style(name, &p, &c);
            }
        }

        Theme { scopes, chrome }
    }

    /// The rendered `Style` for `id` — the markdown/code-token equivalent
    /// of `chrome`'s named fields. Falls back to a plain default `Style`
    /// for an id past this theme's `scopes` length (a future tree-sitter
    /// producer, WP5, may register scopes a theme built before it existed
    /// doesn't know about yet — §1.3: degrade to unstyled text, never
    /// panic or index out of bounds).
    pub fn scope_style(&self, id: ScopeId) -> Style {
        self.scopes.get(id.0 as usize).copied().unwrap_or_default()
    }
}

/// WP4.S2's canonical scope -> `Style` mapping (Catppuccin Mocha). Heading
/// levels step down in colour and weight from `1` to `6`, mirroring the
/// pre-WP4 styling's own "H1-H3 bold and colourful, H4 not bold, H6
/// dimmest" shape without reusing its exact indexed literals. Composite
/// emphasis (plan WP4.S2: "resolves to its strongest component") never
/// reaches here as a combined tag — `rune-md`'s `StyleCtx::resolve` already
/// picked the single strongest scope before tagging the span, so this
/// match only ever sees one emphasis kind at a time.
fn markdown_scope_style(name: &str, p: &Mocha, c: &impl Fn(Color) -> Color) -> Style {
    let base = Style::default();
    match name {
        "text" => base.fg(c(p.text)),
        "markup.heading.1" => base
            .fg(c(p.crust))
            .bg(c(p.red))
            .add_modifier(Modifier::BOLD),
        "markup.heading.2" => base.fg(c(p.peach)).add_modifier(Modifier::BOLD),
        "markup.heading.3" => base.fg(c(p.yellow)).add_modifier(Modifier::BOLD),
        "markup.heading.4" => base.fg(c(p.green)),
        "markup.heading.5" => base.fg(c(p.teal)),
        "markup.heading.6" => base.fg(c(p.overlay1)),
        "markup.strong" => base.add_modifier(Modifier::BOLD),
        "markup.italic" => base.add_modifier(Modifier::ITALIC),
        "markup.strikethrough" => base.add_modifier(Modifier::CROSSED_OUT),
        "markup.raw.inline" => base.fg(c(p.peach)).bg(c(p.surface0)),
        "markup.raw.block" => base.fg(c(p.text)).bg(c(p.mantle)),
        "markup.link" => base.fg(c(p.blue)).add_modifier(Modifier::UNDERLINED),
        "markup.quote" => base.fg(c(p.overlay1)).add_modifier(Modifier::ITALIC),
        "markup.list" => base.fg(c(p.overlay1)),
        "markup.list.checked" => base.fg(c(p.green)),
        // Raw ANSI-256 indices, deliberately NOT routed through the
        // Catppuccin palette like every scope above. Table chrome has to
        // match the Go reference's own table styles byte-for-byte in both
        // truecolor and quantized rendering — the index IS the spec here,
        // and a hue-derived approximation would break screen parity.
        "markup.table.header" => base.fg(Color::Indexed(252)).add_modifier(Modifier::BOLD),
        "markup.table" => base.fg(Color::Indexed(252)),
        "markup.table.separator" => base.fg(Color::Indexed(240)),
        "markup.table.border" => base.fg(Color::Indexed(240)),
        "punctuation.special" => base.fg(c(p.overlay0)),
        "comment" => base.fg(c(p.overlay1)),
        // Unreachable in practice: `name` is always drawn from this same
        // table's own `MARKDOWN_SCOPES` (the loop in
        // `Theme::catppuccin_mocha` walks `table.iter()`), so every arm
        // above is exhaustive over the names that ever reach here — a
        // future scope this match hasn't been taught yet degrades to
        // plain, unstyled text (§1.3) rather than panicking.
        _ => base,
    }
}

/// [`rune_syntax::scope::CODE_SCOPES`]'s canonical scope -> `Style` mapping
/// (Catppuccin Mocha), for tokens a tree-sitter producer tags. Sets only
/// `fg` and `Modifier` — never `bg` — because a later render pass
/// `Style::patch`es this onto a cell that may already carry a
/// `markup.raw.block` background, and a `bg` here would clobber it. Every
/// colour goes through `c(..)` so the quantized (256-colour) construction
/// path never surfaces a raw `Color::Rgb`.
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
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => {
            base.fg(c(p.overlay1))
        }
        "attribute" => base.fg(c(p.yellow)),
        "label" => base.fg(c(p.sapphire)),
        "tag" => base.fg(c(p.blue)),
        // Unreachable in practice: `name` is always drawn from this same
        // table's own `CODE_SCOPES`, so every arm above is exhaustive over
        // the names that ever reach here — a future scope this match
        // hasn't been taught yet degrades to plain, unstyled text (§1.3)
        // rather than panicking.
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_scope_gets_a_non_default_or_deliberately_plain_style() {
        // Every markdown scope resolves to SOME entry in `scopes` — no
        // panic, no out-of-bounds — for both the truecolor and quantized
        // construction paths.
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
}
