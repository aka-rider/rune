//! Semantic style tags and the emphasis-nesting resolver (plan Context,
//! "Nested styling ... falls out of the tree via the Emitter's style
//! stack — no `InlineMarks` bitfield").

use crate::element::inline::EmphasisKind;

/// Semantic style tag — "what kind of markdown token is this", not a
/// rendered `ratatui::Style`. The lipgloss/ratatui-equivalent theme lives in
/// rune-tui (plan Context: "the lipgloss-equivalent theme lives in
/// rune-tui").
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

pub(crate) fn heading_style(level: u8) -> StyleId {
    match level {
        1 => StyleId::H1,
        2 => StyleId::H2,
        3 => StyleId::H3,
        4 => StyleId::H4,
        5 => StyleId::H5,
        _ => StyleId::H6,
    }
}

pub(crate) fn list_marker_style(has_task: bool) -> StyleId {
    if has_task {
        StyleId::TaskMarker
    } else {
        StyleId::ListMarker
    }
}

pub(crate) fn verbatim_style() -> StyleId {
    StyleId::Verbatim
}

/// Per-parent accumulator resolving nested emphasis to one `StyleId` at leaf
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
    Override(StyleId),
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

    pub(crate) fn resolve(self) -> StyleId {
        match self {
            StyleCtx::Override(s) => s,
            StyleCtx::Emphasis {
                bold,
                italic,
                strike,
            } => match (bold, italic, strike) {
                (false, false, false) => StyleId::Text,
                (true, false, false) => StyleId::Bold,
                (false, true, false) => StyleId::Italic,
                (false, false, true) => StyleId::Strike,
                (true, true, false) => StyleId::BoldItalic,
                (true, false, true) => StyleId::BoldStrike,
                (false, true, true) => StyleId::ItalicStrike,
                (true, true, true) => StyleId::BoldItalicStrike,
            },
        }
    }
}
