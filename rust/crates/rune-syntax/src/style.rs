//! Semantic style tags — "what kind of markdown/code token is this", not a
//! rendered `ratatui::Style`. The lipgloss/ratatui-equivalent theme lives in
//! rune-tui (plan Context: "the lipgloss-equivalent theme lives in
//! rune-tui"). Producer-agnostic (WP3): a future tree-sitter producer tags
//! its own spans with the same `StyleId` without depending on `rune-md`.
//! WP4 replaces this with a `ScopeId` resolved against a shared scope
//! namespace.

/// Semantic style tag — "what kind of markdown token is this", not a
/// rendered `ratatui::Style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleId {
    Text,
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Bold,
    Italic,
    BoldItalic,
    Strike,
    BoldStrike,
    ItalicStrike,
    BoldItalicStrike,
    Code,
    CodeFence,
    Link,
    WikiLink,
    Blockquote,
    ListMarker,
    TaskMarker,
    Hr,
    FrontmatterDim,
    Verbatim,
}
