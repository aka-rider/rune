//! Glyph sets for line decorations (heading icons, list bullets, quote
//! bars, thematic-break rules). Plain data only: this crate stays
//! terminal-free, so choosing WHICH set applies is the caller's job.

/// One coherent set of decoration glyphs. `headings` is indexed by
/// heading level minus one; `bullets` cycles by list-nesting depth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconSet {
    pub headings: [&'static str; 6],
    pub bullets: [&'static str; 4],
    pub quote_bar: &'static str,
    pub rule: &'static str,
}

impl IconSet {
    /// Nerd-font tier: Material Design heading icons (a digit inside a
    /// rounded box), private-use-area codepoints that require a Nerd Font
    /// or a terminal with system font fallback.
    pub fn nerd() -> IconSet {
        IconSet {
            headings: [
                "\u{F0CA1} ",
                "\u{F0CA3} ",
                "\u{F0CA5} ",
                "\u{F0CA7} ",
                "\u{F0CA9} ",
                "\u{F0CAB} ",
            ],
            bullets: ["\u{25CF}", "\u{25CB}", "\u{25C6}", "\u{25C7}"],
            quote_bar: "\u{258B}",
            rule: "\u{2500}",
        }
    }

    /// Plain-Unicode tier: BMP-only glyphs that render in any terminal
    /// font. The default when nerd-font support is unknown.
    pub fn unicode() -> IconSet {
        IconSet {
            headings: [
                "\u{25C9} ",
                "\u{25C8} ",
                "\u{25C6} ",
                "\u{25C7} ",
                "\u{25B8} ",
                "\u{25B9} ",
            ],
            bullets: ["\u{2022}", "\u{25E6}", "\u{25AA}", "\u{25AB}"],
            quote_bar: "\u{258E}",
            rule: "\u{2500}",
        }
    }
}
