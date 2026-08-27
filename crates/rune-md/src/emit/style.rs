//! The emphasis-nesting resolver (plan Context, "Nested styling ... falls
//! out of the tree via the Emitter's style stack — no `InlineMarks`
//! bitfield").

use crate::element::inline::EmphasisKind;
use rune_syntax::ScopeId;
use rune_syntax::kind::DocumentKind;
use rune_syntax::scope::{IMAGE_SCOPE_ID, MarkdownScope};

pub(crate) fn heading_style(level: u8) -> ScopeId {
    match level {
        1 => MarkdownScope::Heading1.into(),
        2 => MarkdownScope::Heading2.into(),
        3 => MarkdownScope::Heading3.into(),
        4 => MarkdownScope::Heading4.into(),
        5 => MarkdownScope::Heading5.into(),
        _ => MarkdownScope::Heading6.into(),
    }
}

pub(crate) fn list_marker_style(has_task: bool) -> ScopeId {
    if has_task {
        MarkdownScope::ListChecked.into()
    } else {
        MarkdownScope::List.into()
    }
}

pub(crate) fn verbatim_style() -> ScopeId {
    MarkdownScope::Text.into()
}

/// The plain-text scope — `fill_gaps`' per-byte safety net (`emit::mod`)
/// tags every gap-filled span with this, same as `verbatim_style` above.
pub(crate) fn text_scope() -> ScopeId {
    MarkdownScope::Text.into()
}

/// A fenced code block's body text (`walk.rs::emit_code_fence` pushes every
/// content line at this one scope).
pub(crate) fn code_fence_scope() -> ScopeId {
    MarkdownScope::RawBlock.into()
}

/// The scope a document's UNCLAIMED bytes fall back to, chosen by the
/// document's own kind. This exists so that "this text is code" is a
/// STRUCTURAL fact rather than a coincidence of two theme entries happening
/// to carry similar styles: a whole `.ts` file and a ```` ```ts ```` fence
/// body are the same thing to the reader, so they must resolve to the same
/// scope — the one `code_fence_scope` already names — and can never drift
/// apart under a theme edit.
///
/// A `Code` document parses to an EMPTY block list, so every one of its
/// spans comes from the gap-fill pass; that pass is the only thing this
/// choice reaches. `Plain` deliberately stays `text`: an unrecognized
/// `.txt` is prose of unknown shape, not code.
pub(crate) fn base_scope(kind: DocumentKind) -> ScopeId {
    match kind {
        DocumentKind::Code(_) => code_fence_scope(),
        DocumentKind::Markdown | DocumentKind::Plain | DocumentKind::Image => text_scope(),
    }
}

/// An inline code span (`` `like this` ``).
pub(crate) fn code_scope() -> ScopeId {
    MarkdownScope::RawInline.into()
}

/// A link's visible label. `WikiLink` has no separate scope of its own —
/// same as the pre-WP4 `StyleId` mapping, where `WikiLink` shared `Link`'s
/// style — so it resolves through this same function too.
pub(crate) fn link_scope() -> ScopeId {
    MarkdownScope::Link.into()
}

pub(crate) fn blockquote_scope() -> ScopeId {
    MarkdownScope::Quote.into()
}

/// A blockquote marker's DECOR bar (plan WP2.S5) — distinct from
/// `blockquote_scope`, which styles the quote's own concealed-marker span
/// when Revealed; the bar is the decor-channel glyph a Rendered quote line
/// carries instead.
pub(crate) fn quote_marker_scope() -> ScopeId {
    MarkdownScope::QuoteMarker.into()
}

/// A rendered table's body-row base scope (WP2.S1's `markup.table`) —
/// `table::render::render_cell`'s substitute for plain (non-emphasized)
/// cell text, and `table::layout::grid_row`'s padding scope for a body row.
pub(crate) fn table_scope() -> ScopeId {
    MarkdownScope::Table.into()
}

/// A rendered table's header-row base scope.
pub(crate) fn table_header_scope() -> ScopeId {
    MarkdownScope::TableHeader.into()
}

/// The synthesised delimiter-replacing separator row (`├───┼───┤`) —
/// `table::layout::separator_row`'s one scope for every char, corners and
/// fill alike.
pub(crate) fn table_separator_scope() -> ScopeId {
    MarkdownScope::TableSeparator.into()
}

/// A Grid row's `│` column borders and side padding — `table::layout::
/// grid_row`'s bar scope specifically (padding uses the row's own role
/// scope instead, see that function's docs).
pub(crate) fn table_border_scope() -> ScopeId {
    MarkdownScope::TableBorder.into()
}

pub(crate) fn hr_scope() -> ScopeId {
    MarkdownScope::PunctuationSpecial.into()
}

/// An image's visible label — its alt text (or target when alt is empty) in
/// Rendered state, its raw markup in Revealed state (`walk_inline.rs`'s
/// `Inline::Image` arm). Registered after `CODE_SCOPES`
/// (`rune_syntax::scope::EXTENDED_SCOPES`), not folded into either earlier
/// scope table, so it can't renumber any id both sides of the shared
/// `scope_table()` constructor already agree on.
pub(crate) fn image_scope() -> ScopeId {
    IMAGE_SCOPE_ID
}

/// Frontmatter's `---` DELIMITER lines get their own dim, de-emphasized
/// tone (the pre-WP4 choice), now expressed as the `comment` scope. The body
/// between them is a code region and uses `code_fence_scope` like any other
/// code.
pub(crate) fn frontmatter_scope() -> ScopeId {
    MarkdownScope::Comment.into()
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
                (false, false, false) => MarkdownScope::Text.into(),
                (true, _, _) => MarkdownScope::Strong.into(),
                (false, true, _) => MarkdownScope::Italic.into(),
                (false, false, true) => MarkdownScope::Strikethrough.into(),
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rune_syntax::LangId;

    /// The identity this module exists to make structural: a code
    /// DOCUMENT's text and a fenced code BLOCK's body are one scope, not
    /// two that a theme edit could drift apart. Asserted against
    /// `code_fence_scope` rather than the literal `"markup.raw.block"` on
    /// purpose — re-pinning the name here would recreate exactly the
    /// duplication this removes.
    #[test]
    fn a_code_document_shares_the_code_fence_scope() {
        let rust = LangId::from_name("rust").unwrap();
        assert_eq!(base_scope(DocumentKind::Code(rust)), code_fence_scope());
    }

    /// Everything else falls back to prose. `Plain` is the interesting
    /// one: an unrecognized file is not code just because we failed to
    /// name a language for it.
    #[test]
    fn every_other_kind_falls_back_to_text() {
        assert_eq!(base_scope(DocumentKind::Markdown), text_scope());
        assert_eq!(base_scope(DocumentKind::Plain), text_scope());
        assert_eq!(base_scope(DocumentKind::Image), text_scope());
    }

    /// Each level 1..=5 has its OWN arm; deleting any one of them falls
    /// through to the `_` catch-all and silently mislabels that level as
    /// `Heading6`. Level 6 (and the catch-all it actually exercises) is
    /// included for completeness, though it can't itself distinguish a
    /// deleted arm from an intact one.
    #[test]
    fn heading_style_maps_each_level_to_its_own_distinct_scope() {
        assert_eq!(heading_style(1), MarkdownScope::Heading1.into());
        assert_eq!(heading_style(2), MarkdownScope::Heading2.into());
        assert_eq!(heading_style(3), MarkdownScope::Heading3.into());
        assert_eq!(heading_style(4), MarkdownScope::Heading4.into());
        assert_eq!(heading_style(5), MarkdownScope::Heading5.into());
        assert_eq!(heading_style(6), MarkdownScope::Heading6.into());
    }
}
