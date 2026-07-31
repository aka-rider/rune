//! The emphasis-nesting resolver (plan Context, "Nested styling ... falls
//! out of the tree via the Emitter's style stack — no `InlineMarks`
//! bitfield") plus WP4.S2's `StyleId` -> canonical scope name mapping: every
//! markdown token this emitter tags resolves against the shared
//! [`rune_syntax::scope::scope_table`] (via [`SCOPES`] below) rather than
//! a closed enum variant, so an unstyled scope degrades through
//! longest-dotted-prefix fallback instead of failing to compile.

use std::sync::LazyLock;

use crate::element::inline::EmphasisKind;
use rune_syntax::ScopeId;
use rune_syntax::scope::{ScopeTable, scope_table};

/// The one canonical scope table this emitter resolves every markdown token
/// against — built once (`rune_syntax::scope::scope_table`, WP4.S1/S2),
/// shared by every `sync` call. `rune-tui`'s `Theme` walks a table built
/// from the exact same constructor to size and fill its `scopes: Vec<Style>`
/// — the two sides agree on which `ScopeId` means which name without either
/// depending on the other.
pub static SCOPES: LazyLock<ScopeTable> = LazyLock::new(scope_table);

/// Resolves `name` against [`SCOPES`]. Every name passed below is drawn
/// verbatim from `rune_syntax::scope::MARKDOWN_SCOPES` or (since WP7)
/// `rune_syntax::scope::EXTENDED_SCOPES`, so resolution always succeeds
/// through `ScopeTable::resolve`'s exact-match branch — the `unwrap_or`
/// fallback to `ScopeId(0)` (`"text"`, registered first) exists only so a
/// future typo here degrades gracefully (§1.3) instead of panicking, never
/// because it's expected to fire.
fn scope(name: &str) -> ScopeId {
    SCOPES.resolve(name).unwrap_or(ScopeId(0))
}

pub(crate) fn heading_style(level: u8) -> ScopeId {
    match level {
        1 => scope("markup.heading.1"),
        2 => scope("markup.heading.2"),
        3 => scope("markup.heading.3"),
        4 => scope("markup.heading.4"),
        5 => scope("markup.heading.5"),
        _ => scope("markup.heading.6"),
    }
}

pub(crate) fn list_marker_style(has_task: bool) -> ScopeId {
    if has_task {
        scope("markup.list.checked")
    } else {
        scope("markup.list")
    }
}

pub(crate) fn verbatim_style() -> ScopeId {
    scope("text")
}

/// The plain-text scope — `fill_gaps`' per-byte safety net (`emit::mod`)
/// tags every gap-filled span with this, same as `verbatim_style` above.
pub(crate) fn text_scope() -> ScopeId {
    scope("text")
}

/// A fenced code block's body text (`walk.rs::emit_code_fence` pushes every
/// content line at this one scope).
pub(crate) fn code_fence_scope() -> ScopeId {
    scope("markup.raw.block")
}

/// An inline code span (`` `like this` ``).
pub(crate) fn code_scope() -> ScopeId {
    scope("markup.raw.inline")
}

/// A link's visible label. `WikiLink` has no separate scope of its own —
/// same as the pre-WP4 `StyleId` mapping, where `WikiLink` shared `Link`'s
/// style — so it resolves through this same function too.
pub(crate) fn link_scope() -> ScopeId {
    scope("markup.link")
}

pub(crate) fn blockquote_scope() -> ScopeId {
    scope("markup.quote")
}

/// A blockquote marker's DECOR bar (plan WP2.S5) — distinct from
/// `blockquote_scope`, which styles the quote's own concealed-marker span
/// when Revealed; the bar is the decor-channel glyph a Rendered quote line
/// carries instead.
pub(crate) fn quote_marker_scope() -> ScopeId {
    scope("markup.quote.marker")
}

/// A rendered table's body-row base scope (WP2.S1's `markup.table`) —
/// `table::render::render_cell`'s substitute for plain (non-emphasized)
/// cell text, and `table::layout::grid_row`'s padding scope for a body row.
pub(crate) fn table_scope() -> ScopeId {
    scope("markup.table")
}

/// A rendered table's header-row base scope.
pub(crate) fn table_header_scope() -> ScopeId {
    scope("markup.table.header")
}

/// The synthesised delimiter-replacing separator row (`├───┼───┤`) —
/// `table::layout::separator_row`'s one scope for every char, corners and
/// fill alike.
pub(crate) fn table_separator_scope() -> ScopeId {
    scope("markup.table.separator")
}

/// A Grid row's `│` column borders and side padding — `table::layout::
/// grid_row`'s bar scope specifically (padding uses the row's own role
/// scope instead, see that function's docs).
pub(crate) fn table_border_scope() -> ScopeId {
    scope("markup.table.border")
}

pub(crate) fn hr_scope() -> ScopeId {
    scope("punctuation.special")
}

/// An image's visible label — its alt text (or target when alt is empty) in
/// Rendered state, its raw markup in Revealed state (`walk_inline.rs`'s
/// `Inline::Image` arm). Registered after `CODE_SCOPES`
/// (`rune_syntax::scope::EXTENDED_SCOPES`), not folded into either earlier
/// scope table, so it can't renumber any id both sides of the shared
/// `scope_table()` constructor already agree on.
pub(crate) fn image_scope() -> ScopeId {
    scope("markup.image")
}

/// No Go equivalent (Go doesn't style frontmatter separately) — kept at the
/// pre-WP4 choice of a dim, de-emphasized tone, now expressed as the
/// `comment` scope.
pub(crate) fn frontmatter_scope() -> ScopeId {
    scope("comment")
}

/// Per-parent accumulator resolving nested emphasis to one `ScopeId` at leaf
/// emission time — the "style stack", kept only for the duration of the
/// walk. A non-emphasis ancestor (`Link`/`WikiLink`/`Code`) overrides and
/// ignores any accumulated emphasis (Phase-1 simplification: a link's own
/// color wins over surrounding bold/italic).
#[derive(Clone, Copy, Debug)]
pub(crate) enum StyleCtx {
    Emphasis {
        bold: bool,
        italic: bool,
        strike: bool,
    },
    Override(ScopeId),
}

impl Default for StyleCtx {
    fn default() -> Self {
        StyleCtx::Emphasis {
            bold: false,
            italic: false,
            strike: false,
        }
    }
}

impl StyleCtx {
    pub(crate) fn with_kind(self, kind: EmphasisKind) -> StyleCtx {
        match self {
            StyleCtx::Override(_) => self,
            StyleCtx::Emphasis {
                bold,
                italic,
                strike,
            } => {
                let (bold, italic, strike) = match kind {
                    EmphasisKind::Bold => (true, italic, strike),
                    EmphasisKind::Italic => (bold, true, strike),
                    EmphasisKind::Strike => (bold, italic, true),
                    EmphasisKind::BoldItalic => (true, true, strike),
                };
                StyleCtx::Emphasis {
                    bold,
                    italic,
                    strike,
                }
            }
        }
    }

    /// Resolves accumulated emphasis to one `ScopeId` (plan WP4.S2:
    /// "Composite emphasis resolves to its strongest component with
    /// modifiers carried on the theme entry") — the closed `StyleId` this
    /// replaces had four dedicated composite variants
    /// (`BoldItalic`/`BoldStrike`/`ItalicStrike`/`BoldItalicStrike`); the
    /// open scope namespace has no such combinations, so a span nesting
    /// more than one emphasis kind is tagged with its SINGLE strongest
    /// component's scope (bold > italic > strikethrough) and nothing else —
    /// an accepted, documented simplification, not a bug: a future scope
    /// like `markup.strong.italic` could restore the distinction without
    /// touching this resolver's shape.
    pub(crate) fn resolve(self) -> ScopeId {
        match self {
            StyleCtx::Override(s) => s,
            StyleCtx::Emphasis {
                bold,
                italic,
                strike,
            } => match (bold, italic, strike) {
                (false, false, false) => scope("text"),
                (true, _, _) => scope("markup.strong"),
                (false, true, _) => scope("markup.italic"),
                (false, false, true) => scope("markup.strikethrough"),
            },
        }
    }
}
