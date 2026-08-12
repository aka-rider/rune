//! `DisplaySnapshot::expand_images` (plan WP8) — `expand_tables`'s sibling,
//! chained after it: reserves rows for a standalone inline image embed
//! (`![alt](url)`/`![[target]]` alone on its own line, plan WP7's
//! `element::inline::standalone_image`) exactly the way `expand_tables`
//! reserves rows for a table's synthetic borders. Split into its own file
//! from `super` to stay under the 500-line budget — both files reach `DisplaySnapshot`'s
//! private fields, a child module already seeing its parent's private
//! items, so `expand_images` builds `DisplaySnapshot { rows, wrap_to_display
//! }` here exactly as `expand_tables` does in `super`.

use std::collections::HashMap;

use rune_syntax::SyntaxSpan;
use rune_syntax::wrap::WrapSnapshot;

use crate::element::block::Block;
use crate::element::inline::{ImageM, standalone_image};
use crate::emit::style::image_scope;

use super::{DisplayRow, DisplaySnapshot, ImageRowRef};

/// Per-embed cell footprint `(cols, rows)`, threaded in from `rune-tui`
/// (plan WP8.S3) — the only crate that knows terminal cell-pixel geometry.
/// Keyed by an image target's resolved lookup key; today that is the raw
/// `ImageM::target_text` an embed carries (path resolution against the
/// document's directory/workspace root is `rune-tui`'s job, WP9 — this map
/// only ever holds whatever key the caller resolved to). `rune-md` stays
/// terminal-free: no colour, no protocol, no `rune-tui` type appears here,
/// mirroring `ImageRowRef`'s own doc comment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageDims(HashMap<String, (usize, usize)>);

impl ImageDims {
    pub fn new() -> ImageDims {
        ImageDims::default()
    }

    pub fn insert(&mut self, key: impl Into<String>, cols: usize, rows: usize) {
        self.0.insert(key.into(), (cols, rows));
    }

    pub fn get(&self, key: &str) -> Option<(usize, usize)> {
        self.0.get(key).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl DisplaySnapshot {
    /// `expand_tables`'s sibling (plan WP8), chained after it: inserts
    /// `rows - 1` synthetic continuation rows after every standalone-image
    /// anchor row, so an inline embed reserves the same vertical space in
    /// `total_rows()` an image document's own producer reserves for a whole
    /// picture (WP4.S2's shape — `synthetic: true`, `wrap_row` borrowed from
    /// the anchor, a single `Substituted` span whose `cell_map` is all `-1`
    /// and whose range is empty).
    ///
    /// `blocks`/`content` locate WHICH lines are standalone image lines
    /// (`element::inline::standalone_image`, walked recursively through
    /// paragraphs and list items — the only containers a bare `![alt](url)`/
    /// `![[target]]` line can sit directly inside); `dims` says how big a
    /// located image's footprint is. The two are deliberately separate: an
    /// image with a target `dims` has never heard of (no dimensions probed
    /// yet, or decode still in flight) still reserves its row — exactly 1,
    /// this pass's default — so the layout doesn't jump once dimensions
    /// arrive (plan WP8.S4, mirroring the reference's "reserve as soon as
    /// decode completes").
    pub fn expand_images(
        self,
        wrap: &WrapSnapshot,
        blocks: &[Block],
        content: &str,
        dims: &ImageDims,
    ) -> DisplaySnapshot {
        let mut anchors: HashMap<usize, &ImageM> = HashMap::new();
        collect_standalone_images(blocks, content, &mut anchors);

        let segments = wrap.segments();
        let mut rows: Vec<DisplayRow> = Vec::with_capacity(self.rows.len());
        let mut wrap_to_display = vec![0usize; segments.len()];

        for mut row in self.rows {
            let wrap_row = row.wrap_row;
            if !row.synthetic
                && let Some(slot) = wrap_to_display.get_mut(wrap_row)
            {
                *slot = rows.len();
            }

            // Only the model line's OWN first wrap row can be the anchor —
            // a later wrap row of the same over-wide line (the image's
            // label text wrapping) must never re-trigger a second
            // reservation for the same embed.
            let model_line = segments.get(wrap_row).map(|s| s.model_line);
            let target = (!row.synthetic)
                .then_some(model_line)
                .flatten()
                .filter(|&ml| wrap.model_line_to_first_row(ml) == wrap_row)
                .and_then(|ml| anchors.get(&ml).copied());

            if let Some(target) = target {
                let target_text = target.target_text.as_str();
                let (width, image_rows) = dims
                    .get(target_text)
                    .map(|(cols, rows)| (cols, rows.max(1)))
                    .unwrap_or((1, 1));
                row.image = Some(ImageRowRef {
                    row: 0,
                    width,
                    target: Some(target_text.to_string()),
                });
                rows.push(row);
                for i in 1..image_rows {
                    rows.push(synthetic_image_row(wrap_row, i, width, target_text));
                }
            } else {
                rows.push(row);
            }
        }

        DisplaySnapshot {
            rows,
            wrap_to_display,
        }
    }
}

/// A continuation row of a multi-row image — `expand_images`'s own
/// `synthetic_border` counterpart. `width` many space chars stand in for
/// the row's real content; the renderer never reads this text (it builds
/// placeholder cells straight from `DisplayRow::image` instead, WP4/WP9),
/// so only the char COUNT — matching `cell_map`'s length, same invariant
/// `synthetic_border` keeps — has to be right.
fn synthetic_image_row(
    wrap_row: usize,
    image_row: usize,
    width: usize,
    target: &str,
) -> DisplayRow {
    let text = " ".repeat(width);
    let cell_map = vec![None; text.chars().count()];
    let span = SyntaxSpan::substituted_mapped(image_scope(), text, 0..0, cell_map);
    DisplayRow {
        spans: vec![span],
        wrap_row,
        synthetic: true,
        decor: None,
        image: Some(ImageRowRef {
            row: image_row,
            width,
            target: Some(target.to_string()),
        }),
    }
}

/// Every standalone-image line reachable from `blocks`, keyed by model
/// line (`element::inline::ImageM::line`, the same indexing
/// `WrapSegment::model_line` uses) — recurses into the only two container
/// kinds a bare image line can sit directly inside (a blockquote's own
/// paragraphs, a list item's children); every other block kind (headings,
/// tables, fences, ...) cannot contain a standalone image line by
/// construction, so is skipped rather than mis-walked. `pub` (not just
/// crate-private): `rune-tui`'s own embed reconciler (plan WP9.S4) needs
/// the exact same "which lines spawn an embed" answer this pass already
/// computes — a second, independently-written walk over the same block
/// shapes would drift the moment one of them added a container kind the
/// other didn't. Keyed by the whole `&ImageM`, not just its `target_text`
/// (as an earlier revision did), so a caller can also read `is_wikilink`
/// (needed to resolve `![[target]]` differently from `![alt](url)`, plan
/// WP9.S7) without a second walk.
pub fn collect_standalone_images<'a>(
    blocks: &'a [Block],
    content: &str,
    out: &mut HashMap<usize, &'a ImageM>,
) {
    for block in blocks {
        match block {
            Block::Paragraph(p) => {
                for img in standalone_image(content, &p.inlines) {
                    out.insert(img.line, img);
                }
            }
            Block::Blockquote(bq) => collect_standalone_images(&bq.children, content, out),
            Block::List(list) => {
                for item in &list.items {
                    collect_standalone_images(&item.children, content, out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::snapshot::assert_round_trip_and_synthetic_adjacency;
    use rune_syntax::wrap::WrapMap;

    /// WP8.S1: the same round-trip/adjacency invariant, now over a document
    /// whose table AND standalone image line both insert synthetic rows —
    /// written and passing BEFORE `expand_images` existed (plan Gotchas:
    /// "missing either conversion scrolls documents wrong by the number of
    /// inserted rows above the cursor" — proven here for image rows exactly
    /// as it already was for table borders).
    #[test]
    fn wrap_display_round_trip_covers_image_rows() {
        let content =
            "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n\n![alt](x.png)\n\ntrailing text";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        let wrap = WrapMap::new(80).sync(content, &lines);
        let mut dims = ImageDims::new();
        dims.insert("x.png", 10, 4);
        let display = DisplaySnapshot::from_wrap(&wrap)
            .expand_tables(&wrap)
            .expand_images(&wrap, &blocks, content, &dims);

        assert!(
            display.rows().iter().any(|r| r.image.is_some()),
            "fixture must actually reserve image rows"
        );
        assert_round_trip_and_synthetic_adjacency(&wrap, &display);
    }

    /// WP8.S2/S4: a standalone image line with a known 5-row footprint
    /// produces exactly 5 display rows for its one model line — an anchor
    /// row plus 4 synthetic continuations sharing the anchor's `wrap_row`.
    #[test]
    fn standalone_image_with_five_rows_produces_five_display_rows() {
        let content = "![alt](x.png)";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        let wrap = WrapMap::new(80).sync(content, &lines);
        assert_eq!(wrap.total_rows(), 1, "fixture is a single model line");
        let mut dims = ImageDims::new();
        dims.insert("x.png", 12, 5);
        let display = DisplaySnapshot::from_wrap(&wrap)
            .expand_tables(&wrap)
            .expand_images(&wrap, &blocks, content, &dims);

        assert_eq!(display.total_rows(), 5);
        let image_rows: Vec<_> = display
            .rows()
            .iter()
            .filter_map(|r| r.image.as_ref())
            .collect();
        assert_eq!(image_rows.len(), 5);
        for (i, img) in image_rows.iter().enumerate() {
            assert_eq!(img.row, i);
            assert_eq!(img.width, 12);
        }
        assert!(!display.rows()[0].synthetic, "the anchor row is real");
        assert!(
            display.rows()[1..].iter().all(|r| r.synthetic),
            "every continuation row is synthetic"
        );
        assert!(
            display.rows().iter().all(|r| r.wrap_row == 0),
            "every image row borrows the anchor's own wrap_row"
        );
    }

    /// WP8 "Done when": `wrap_to_display` never returns a synthetic image
    /// row's index — the caret must be able to land only on the anchor.
    #[test]
    fn wrap_to_display_never_returns_a_synthetic_image_row() {
        let content = "![alt](x.png)";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        let wrap = WrapMap::new(80).sync(content, &lines);
        let mut dims = ImageDims::new();
        dims.insert("x.png", 12, 5);
        let display = DisplaySnapshot::from_wrap(&wrap)
            .expand_tables(&wrap)
            .expand_images(&wrap, &blocks, content, &dims);

        for w in 0..wrap.total_rows() {
            let d = display.wrap_to_display(w);
            assert!(!display.rows()[d].synthetic);
            assert_eq!(display.rows()[d].image.as_ref().map(|i| i.row), Some(0));
        }
    }

    /// WP8.S4: an image whose dimensions are unknown (absent from `dims`)
    /// still reserves — exactly 1 row, so the layout doesn't jump once
    /// decode completes and dimensions are learned.
    #[test]
    fn unknown_dimensions_reserve_exactly_one_row() {
        let content = "![alt](x.png)";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        let wrap = WrapMap::new(80).sync(content, &lines);
        let dims = ImageDims::new();
        let display = DisplaySnapshot::from_wrap(&wrap)
            .expand_tables(&wrap)
            .expand_images(&wrap, &blocks, content, &dims);

        assert_eq!(display.total_rows(), 1);
        assert_eq!(display.rows()[0].image.as_ref().map(|i| i.row), Some(0));
        assert!(!display.rows()[0].synthetic);
    }

    /// A truly-inline image (text before/after it on the same line) is NOT
    /// a standalone image line, so `expand_images` must leave its row count
    /// untouched — `standalone_image` already refuses it; this pins that
    /// `expand_images` actually honours the refusal rather than reserving
    /// rows unconditionally for any line containing an image span.
    #[test]
    fn inline_image_amid_text_reserves_no_extra_rows() {
        let content = "before ![alt](x.png) after";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        let wrap = WrapMap::new(80).sync(content, &lines);
        let mut dims = ImageDims::new();
        dims.insert("x.png", 12, 5);
        let display = DisplaySnapshot::from_wrap(&wrap)
            .expand_tables(&wrap)
            .expand_images(&wrap, &blocks, content, &dims);

        assert_eq!(display.total_rows(), wrap.total_rows());
        assert!(display.rows().iter().all(|r| r.image.is_none()));
    }
}
