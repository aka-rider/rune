//! Producer-selection vocabulary (plan WP4): which producer a document's
//! content goes through. `Markdown` is comrak's; `Code` is a named
//! tree-sitter language (WP5 wires the actual highlight); `Plain` has no
//! producer at all and renders every line verbatim. Lives here, not
//! duplicated in `rune-md` and `rune-tui`, because this crate is already
//! the producer-agnostic layer both depend on.

/// Which producer a document's content is parsed by. `#[default]` is
/// `Markdown` so a document that never explicitly picks a kind (an
/// untitled draft, the help document) keeps today's behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DocumentKind {
    #[default]
    Markdown,
    /// A code document bound to a canonical language name (`rune_ts::lang::
    /// resolve`'s output) — a fenced code block's info string or a file
    /// extension.
    Code(&'static str),
    /// No known producer: rendered verbatim, exactly like `Code`, but with
    /// no language to highlight against.
    Plain,
    /// A read-only image document (plan WP4): content is never comrak- or
    /// verbatim-parsed at all — the producer synthesizes display rows
    /// directly (`rune_md::element::doc::DocMachine`'s `Image` branch)
    /// rather than deriving them from a buffer, since the buffer itself
    /// stays permanently empty (image bytes never live in a `Buffer` — it
    /// is UTF-8 by type).
    Image,
}

impl DocumentKind {
    pub fn is_markdown(&self) -> bool {
        matches!(self, DocumentKind::Markdown)
    }

    /// The language name to highlight against, or `None` for `Markdown`
    /// (comrak owns fenced-code highlighting choices), `Plain` (no language
    /// at all), and `Image` (no text to highlight at all).
    pub fn language(&self) -> Option<&'static str> {
        match self {
            DocumentKind::Code(lang) => Some(lang),
            DocumentKind::Markdown | DocumentKind::Plain | DocumentKind::Image => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_markdown() {
        assert_eq!(DocumentKind::default(), DocumentKind::Markdown);
    }

    #[test]
    fn only_code_carries_a_language() {
        assert_eq!(DocumentKind::Markdown.language(), None);
        assert_eq!(DocumentKind::Plain.language(), None);
        assert_eq!(DocumentKind::Image.language(), None);
        assert_eq!(DocumentKind::Code("rust").language(), Some("rust"));
    }

    #[test]
    fn is_markdown_is_true_only_for_markdown() {
        assert!(DocumentKind::Markdown.is_markdown());
        assert!(!DocumentKind::Code("rust").is_markdown());
        assert!(!DocumentKind::Plain.is_markdown());
        assert!(!DocumentKind::Image.is_markdown());
    }
}
