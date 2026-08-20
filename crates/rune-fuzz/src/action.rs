use rune_syntax::ScopeId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pointer::MouseInput;
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

pub const OPEN_FILESEARCH_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('F'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightVersion {
    Live,
    Stale,
    Future,
}

impl HighlightVersion {
    pub fn resolve(self, live: u64) -> u64 {
        match self {
            HighlightVersion::Live => live,
            HighlightVersion::Stale => live.saturating_sub(1),
            HighlightVersion::Future => live.saturating_add(1),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Key(KeyInput),
    Mouse(MouseInput),
    Type(String),
    Paste(String),
    OpenFileSearch,
    Resize(u16, u16),
    ClipboardReply(String),
    ConfirmTimeout,
    StaleConfirmTimeout(u32),
    Deliver,
    FailNextSave,
    DirLoaded {
        entries: Vec<DirEntry>,
        cause: DirCause,
        generation: u32,
    },
    Highlight {
        version: HighlightVersion,
        spans: Vec<(usize, usize, u16)>,
    },
    DivergeDisk,
    DeliverDb,
    DeliverDbAll,
    HighlightTree {
        version: HighlightVersion,
        fixture: u8,
        base: usize,
    },
    AdvanceClock(u64),
    PaletteRecentsLoaded {
        generation: PaletteGenClaim,
        ok: bool,
        names: Vec<String>,
    },
    InstallDiffLeft {
        seed_index: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteGenClaim {
    Live,
    Stale(u32),
}

impl PaletteGenClaim {
    pub fn resolve(
        &self,
        live: Option<rune_tui::generation::PaletteGen>,
    ) -> rune_tui::generation::PaletteGen {
        match self {
            PaletteGenClaim::Live => {
                live.unwrap_or_else(|| rune_tui::generation::PaletteGen::from_raw(0))
            }
            PaletteGenClaim::Stale(raw) => {
                rune_tui::generation::PaletteGen::from_raw(u64::from(*raw))
            }
        }
    }
}

/// Rebuilds the concrete `(Range<usize>, ScopeId)` pairs `Msg::Highlighted`
/// carries from an `Action::Highlight`'s raw triples — shared by the driver
/// (constructing the real message) and the script codec (round-trip tests
/// never need this, but keeping the conversion in one place avoids two
/// copies of the same `u16 -> ScopeId` wrap).
pub fn highlight_spans_from_raw(
    spans: &[(usize, usize, u16)],
) -> Vec<(std::ops::Range<usize>, ScopeId)> {
    spans
        .iter()
        .map(|&(start, end, scope)| (start..end, ScopeId(scope)))
        .collect()
}

/// Small JSON sources for `Action::HighlightTree`. Grammar is fixed to JSON
/// on purpose — the smallest, most stable tree-sitter grammar with an
/// upstream-maintained highlights query — so this action's in-process
/// grammar-crash exposure stays to exactly one grammar. At least one
/// fixture spans multiple physical lines; at least one contains a
/// multi-byte UTF-8 character, so an out-of-bounds `base` can land a span
/// end off a char boundary downstream.
pub const TREE_FIXTURES: &[&str] = &[
    "{}",
    "{\n  \"a\": 1,\n  \"b\": [true, null]\n}",
    "{\"city\": \"Zürich\", \"emoji\": \"😀\"}",
];

/// Indexes `TREE_FIXTURES` modulo its length, so any `u8` the generator or a
/// decoded script hands in is a valid selector.
pub fn tree_fixture(fixture: u8) -> &'static str {
    let len = TREE_FIXTURES.len();
    TREE_FIXTURES
        .get(fixture as usize % len)
        .copied()
        .unwrap_or("{}")
}

/// Rebuilds a fixture's physical line byte ranges shifted to `base`,
/// matching `LineMap::new`'s reconstruction rule: consecutive lines are
/// separated by exactly one buffer byte (the joining `'\n'`). Saturating
/// arithmetic throughout, so a huge `base` shifts every range to
/// `usize::MAX` instead of overflowing.
pub fn tree_fixture_line_ranges(source: &str, base: usize) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            ranges.push(start..i);
            start = i + 1;
        }
    }
    ranges.push(start..source.len());
    ranges
        .into_iter()
        .map(|r| r.start.saturating_add(base)..r.end.saturating_add(base))
        .collect()
}

/// The parse budget `highlight_tree_reply` hands to `rune_ts::parse` — the
/// fixtures are tiny, so this is never expected to be exhausted; it exists
/// only so a parse can never block indefinitely.
pub const TREE_PARSE_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// Builds the one `Msg::Highlighted`-ready reply `Action::HighlightTree`
/// delivers: a single region whose `LineMap` is `tree_fixture_line_ranges`
/// anchored at `base`, and whose payload is a real `rune_ts::parse` of
/// `tree_fixture(fixture)`, mapped to `RegionPayload::Tree` (a carry-forward
/// on a parse failure, never a panic — the driver and its acceptance test
/// share this one construction so they can never drift apart).
pub fn highlight_tree_reply(fixture: u8, base: usize) -> rune_tui::highlight::HighlightReply {
    let source = tree_fixture(fixture);
    let map = rune_tui::linemap::LineMap::new(source, tree_fixture_line_ranges(source, base));
    let outcome = match rune_ts::parse("json", source, TREE_PARSE_BUDGET) {
        Some(tree) => rune_tui::highlight::RegionOutcome::Replace(
            rune_tui::highlight::RegionPayload::Tree(tree),
        ),
        None => rune_tui::highlight::RegionOutcome::CarryForward {
            source: source.to_string(),
        },
    };
    rune_tui::highlight::HighlightReply {
        regions: vec![rune_tui::highlight::RegionResult { map, outcome }],
        truncated: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_tui::linemap::LineMap;
    use std::time::Duration;

    #[test]
    fn every_fixture_parses_as_json() {
        for &source in TREE_FIXTURES {
            let parsed = rune_ts::parse("json", source, Duration::from_secs(5));
            assert!(
                parsed.is_some(),
                "fixture {source:?} failed to parse as json"
            );
        }
    }

    #[test]
    fn line_ranges_at_base_zero_reconstruct_byte_identically() {
        for &source in TREE_FIXTURES {
            let ranges = tree_fixture_line_ranges(source, 0);
            let map = LineMap::new(source, ranges);
            let reconstructed = map.reconstruct(source);
            assert_eq!(
                reconstructed.as_deref(),
                Some(source),
                "fixture {source:?} did not reconstruct byte-identically at base 0"
            );
        }
    }

    #[test]
    fn line_ranges_shift_by_base_with_saturating_arithmetic() {
        let source = tree_fixture(1);
        let ranges = tree_fixture_line_ranges(source, usize::MAX - 1);
        assert!(!ranges.is_empty());
        for range in &ranges {
            assert!(range.start <= range.end, "inverted range {range:?}");
        }
        let last = ranges.last();
        assert!(
            matches!(last, Some(r) if r.end == usize::MAX || r.end == usize::MAX - 1),
            "expected the shift to saturate near usize::MAX, got {last:?}"
        );
    }

    #[test]
    fn tree_fixture_indexes_modulo_table_length() {
        let len = TREE_FIXTURES.len() as u8;
        assert_eq!(tree_fixture(0), tree_fixture(len));
        assert_eq!(tree_fixture(u8::MAX), tree_fixture(u8::MAX % len));
    }
}
