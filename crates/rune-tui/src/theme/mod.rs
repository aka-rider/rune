//! The rendered theme: the ONE chokepoint every
//! `ratatui::style::Style` in this crate is built from — replaces an
//! earlier `styles.rs`'s 17 independent chrome-style functions plus its
//! `markdown(id)` funnel with a single `Theme` value held on `App`.
//!
//! `scopes` is indexed by the `ScopeId` `rune-syntax`'s `ScopeTable` hands
//! out — `rune-syntax` owns that table; this module only ever maps a
//! resolved id to a `Style`, never registers or resolves a scope name
//! itself. `chrome` covers everything that isn't a
//! markdown/code token: pane borders, tab labels, the footer, etc.
//!
//! Colours are stored as truecolor `Color::Rgb` (Catppuccin Mocha,
//! `catppuccin.rs`) and quantized to `Color::Indexed` exactly once, here at
//! construction (`quantize.rs`), never per frame — macOS Terminal.app (the
//! default terminal on the only OS this app supports) is 256-colour only.

pub mod catppuccin;
pub mod icons;
pub mod probe;
pub mod quantize;

use ratatui::style::{Color, Modifier, Style};

use catppuccin::Mocha;
use quantize::to_ansi256;
use rune_syntax::ScopeId;
use rune_syntax::scope::{CODE_SCOPES, MARKDOWN_SCOPES, scope_table};

/// Every chrome (non-markdown/code) style an earlier `styles.rs` used to
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
    /// The messages pane's `Severity::Error` colour — `Info`
    /// paints no colour of its own (plain text); `Warn` uses this yellow,
    /// unbolded so it reads as less urgent than `error`'s bold red.
    pub warn: Style,
    pub special: Color,
    pub subtle: Color,
    pub selection_bg: Color,
    /// The background painted behind a whole code REGION, as a rectangle,
    /// rather than tagged onto its tokens. It lives here beside
    /// `selection_bg` because it is a region colour, not a token scope: a
    /// span's `bg` can only colour cells that exist, so it leaves a code
    /// block's blank lines and the ragged space past each short line
    /// uncovered. The render module's code-background pass is its one
    /// consumer.
    pub code_bg: Color,
    /// Backgrounds for one unresolved merge block's ours span, theirs span,
    /// and marker lines (`merge::paint` is the only consumer).
    /// Catppuccin-tinted rather than raw ANSI green/red/grey, muted against
    /// `surface0` the same way `code_bg` sits at full `surface0` rather
    /// than a saturated hue.
    pub merge_ours_bg: Style,
    pub merge_theirs_bg: Style,
    pub merge_marker_bg: Style,
    /// The in-file search bar's match highlight. Its own
    /// field rather than reusing `selection_bg`: a match and a live text
    /// selection can both be on screen at once and must read as visually
    /// distinct.
    pub search_match_bg: Style,
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
            warn: Style::new().fg(c(p.yellow)),
            special: c(p.mauve),
            subtle: c(p.overlay1),
            selection_bg: c(p.surface2),
            code_bg: c(p.surface0),
            merge_ours_bg: Style::new().bg(c(blend(p.surface0, p.green, 0.35))),
            merge_theirs_bg: Style::new().bg(c(blend(p.surface0, p.red, 0.35))),
            merge_marker_bg: Style::new().bg(c(p.surface1)),
            search_match_bg: Style::new().bg(c(blend(p.surface0, p.peach, 0.55))),
        };

        let table = scope_table();
        let mut scopes = vec![Style::default(); table.len()];
        for name in MARKDOWN_SCOPES {
            if let Some(id) = table.resolve(name)
                && let Some(slot) = scopes.get_mut(id.0 as usize)
            {
                *slot = markdown_scope_style(name, &p, &c);
            }
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

    /// The rendered `Style` for `id` — the markdown/code-token equivalent
    /// of `chrome`'s named fields. Falls back to a plain default `Style`
    /// for an id past this theme's `scopes` length (a future tree-sitter
    /// producer may register scopes a theme built before it existed
    /// doesn't know about yet — degrade to unstyled text, never
    /// panic or index out of bounds).
    pub fn scope_style(&self, id: ScopeId) -> Style {
        self.scopes.get(id.0 as usize).copied().unwrap_or_default()
    }

    /// [`Theme::scope_style`] with `bg` stripped. An overlay cell's style is
    /// merged onto the render buffer field-wise (`Style::patch` only
    /// touches fields the patching style actually sets), so a scope that
    /// carries a background here would silently overwrite whatever the base
    /// cell had — the code-region background rectangle painted underneath
    /// it, or an inline code span's own `markup.raw.inline` chip. Every
    /// overlay consumer routes through this instead of `scope_style`.
    pub fn overlay_scope_style(&self, id: ScopeId) -> Style {
        Style {
            bg: None,
            ..self.scope_style(id)
        }
    }
}

/// Linearly mixes two truecolor `Color::Rgb` values by `t` (`0.0` is pure
/// `a`, `1.0` is pure `b`) — the same muting `merge_ours_bg`/
/// `merge_theirs_bg` use to tint `surface0` toward green/red rather than
/// painting a whole background in a fully saturated hue. Falls back to `a`
/// unmixed for any non-`Rgb` variant (never reached with `Mocha::palette`'s
/// own colours, all `Rgb`, but kept total rather than partial).
fn blend(a: Color, b: Color, t: f32) -> Color {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return a;
    };
    let mix =
        |x: u8, y: u8| -> u8 { (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8 };
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

/// The canonical scope -> `Style` mapping (Catppuccin Mocha). Heading
/// levels step down in colour and weight from `1` to `6`, mirroring an
/// earlier styling's own "H1-H3 bold and colourful, H4 not bold, H6
/// dimmest" shape without reusing its exact indexed literals. Composite
/// emphasis resolves to its strongest component and never
/// reaches here as a combined tag — `rune-md`'s `StyleCtx::resolve` already
/// picked the single strongest scope before tagging the span, so this
/// match only ever sees one emphasis kind at a time.
fn markdown_scope_style(name: &str, p: &Mocha, c: &impl Fn(Color) -> Color) -> Style {
    let base = Style::default();
    match name {
        "text" => base.fg(c(p.text)),
        "markup.heading.1" => base.fg(c(p.red)).add_modifier(Modifier::BOLD),
        "markup.heading.2" => base.fg(c(p.peach)).add_modifier(Modifier::BOLD),
        "markup.heading.3" => base.fg(c(p.yellow)).add_modifier(Modifier::BOLD),
        "markup.heading.4" => base.fg(c(p.green)),
        "markup.heading.5" => base.fg(c(p.teal)),
        "markup.heading.6" => base.fg(c(p.overlay1)),
        "markup.strong" => base.add_modifier(Modifier::BOLD),
        "markup.italic" => base.add_modifier(Modifier::ITALIC),
        "markup.strikethrough" => base.add_modifier(Modifier::CROSSED_OUT),
        // Sapphire, not peach: peach is `markup.heading.2`, so an inline
        // code span and an H2 title used to render in the same warm
        // orange — code read as structure and cluttered the prose around
        // it. Sapphire is a cold cyan-blue no other markdown scope claims,
        // and stays distinct from `markup.link`'s blue (also underlined)
        // and heading 5's teal. The `surface1` chip is what makes a span
        // legible mid-prose and stays.
        "markup.raw.inline" => base.fg(c(p.sapphire)).bg(c(p.surface1)),
        // Foreground only. A code block's background is a REGION colour
        // (`ChromeStyles::code_bg`), painted as a rectangle by its own
        // render pass: a span's `bg` can only reach cells that exist, so it
        // left a block's blank lines bare and stopped at each short line's
        // last character.
        "markup.raw.block" => base.fg(c(p.text)),
        "markup.link" => base.fg(c(p.blue)).add_modifier(Modifier::UNDERLINED),
        "markup.quote" => base.fg(c(p.overlay1)).add_modifier(Modifier::ITALIC),
        "markup.quote.marker" => base.fg(c(p.overlay0)),
        "markup.list" => base.fg(c(p.overlay1)),
        "markup.list.checked" => base.fg(c(p.green)),
        // Table chrome reads as body text with dimmer rules around it, so
        // it is expressed in palette terms like every scope above rather
        // than as raw ANSI indices. The literals these replace bypassed
        // `c(..)` entirely, which meant the quantized path stayed indexed
        // only by accident of the constants already being indexed.
        "markup.table.header" => base.fg(c(p.text)).add_modifier(Modifier::BOLD),
        "markup.table" => base.fg(c(p.text)),
        "markup.table.separator" => base.fg(c(p.surface2)),
        "markup.table.border" => base.fg(c(p.surface2)),
        "punctuation.special" => base.fg(c(p.overlay0)),
        "comment" => base.fg(c(p.overlay1)),
        // Unreachable in practice: `name` is always drawn from this same
        // table's own `MARKDOWN_SCOPES` (the loop in
        // `Theme::catppuccin_mocha` walks `table.iter()`), so every arm
        // above is exhaustive over the names that ever reach here — a
        // future scope this match hasn't been taught yet degrades to
        // plain, unstyled text rather than panicking.
        _ => base,
    }
}

/// [`rune_syntax::scope::CODE_SCOPES`]'s canonical scope -> `Style` mapping
/// (Catppuccin Mocha), for tokens a tree-sitter producer tags. Sets only
/// `fg` and `Modifier` — never `bg` — because a later render pass
/// `Style::patch`es this onto a cell that already carries the code region's
/// own background rectangle, and a `bg` here would clobber it. Every
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
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => base.fg(c(p.overlay1)),
        "attribute" => base.fg(c(p.yellow)),
        "label" => base.fg(c(p.sapphire)),
        "tag" => base.fg(c(p.blue)),
        // Unreachable in practice: `name` is always drawn from this same
        // table's own `CODE_SCOPES`, so every arm above is exhaustive over
        // the names that ever reach here — a future scope this match
        // hasn't been taught yet degrades to plain, unstyled text
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

    #[test]
    fn overlay_scope_style_never_carries_a_background() {
        // An overlay cell's style is merged onto the
        // render buffer field-wise, so a scope carrying a bg here would
        // clobber whatever background the base cell already had.
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
        // Code is content; a heading is structure. When the two share a
        // foreground the reader cannot tell them apart at a glance —
        // inline code was once byte-identical to `markup.heading.2`, and
        // read as a title mid-sentence. The rule is stated instead of a
        // hex being pinned, so a future palette swap stays free to move
        // any of these colours, just not back on top of each other.
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
    fn code_scopes_never_carry_a_background() {
        // A later render pass `Style::patch`es a code-token style onto a
        // cell that already carries a background: the code region's own
        // rectangle behind every code row, and `markup.raw.inline`'s
        // `surface1` chip wherever a code token is painted over an inline
        // span. `code_scope_style` must never set `bg`, or it would clobber
        // that background instead of layering over it.
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
