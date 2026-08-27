//! `DisplaySnapshot` (plan Context, "Emit -> wrap -> snapshot"): the wrap
//! rows, expanded with the things wrap genuinely cannot do — synthesised
//! top/bottom/inter-row table borders that have no source line at all
//! (architectural decision 3), and, in the sibling `image_rows` module,
//! reserved rows for a standalone inline image embed (plan WP8). `from_wrap`
//! is the identity (one `SnapshotRow` per wrap row); `expand_tables` walks a
//! table's rendered rows and inserts a synthetic border row wherever
//! `TableSegInfo::boundary` says one belongs; `expand_images` chains after
//! it and does the same for a standalone image line. Kept as its own type,
//! distinct from `WrapSnapshot`, so every display-space consumer
//! (`render::build_rows`, the viewport, mouse hit-testing) reads row
//! geometry through ONE place instead of each re-deriving "which wrap row
//! is this synthetic row next to" itself.
//!
//! Split across two files to stay under the 500-line budget: this module owns `SnapshotRow`
//! and the table-border half of `DisplaySnapshot`'s API; `image_rows` owns
//! `ImageDims` and the image half (`expand_images`, its own synthetic-row
//! builder, and the standalone-image-line scan) as a second `impl
//! DisplaySnapshot` block — both files reach `DisplaySnapshot`'s private
//! fields, since a child module already sees its parent's private items.

mod image_rows;

pub use image_rows::{ImageDims, collect_standalone_images};

use rune_core::coords::{DisplayRow, WrapRow};
use rune_syntax::SyntaxSpan;
use rune_syntax::syntax::{RowBoundary, TableRole};
use rune_syntax::wrap::{SegDecor, WrapSnapshot};

use crate::emit::style::table_border_scope;
use crate::table::layout::{BorderKind, border_row};

/// One row of the display grid: either a real wrap row's own spans
/// (`synthetic: false`, `wrap_row` its own row number), or a synthesised
/// table border with no source line at all (`synthetic: true`, `wrap_row`
/// borrowed from the adjacent content row it was inserted next to — see
/// `expand_tables`). Every char of a synthetic row's one span carries
/// `cell_map` entry `None` and an empty `range`: it has no buffer
/// correspondence, decorative through and through.
#[derive(Clone, Debug)]
pub struct SnapshotRow {
    pub spans: Vec<SyntaxSpan>,
    pub wrap_row: usize,
    pub synthetic: bool,
    /// This row's own decoration (heading icon / list bullet / quote bar /
    /// hr rule), carried straight from the wrap segment it was built from —
    /// `None` for an undecorated row and for every synthetic table-border
    /// row (a border has no source line to decorate).
    pub decor: Option<SegDecor>,
    /// Per-row image metadata, the same "carried for the renderer's
    /// benefit" precedent as `decor` — `None` for every row that isn't part
    /// of an image (the image producer/`expand_images` pass are what set
    /// this to `Some`). Keyed off by `rune-tui`'s `build_rows` override to
    /// build placeholder cells instead of the ordinary syntax-span cell
    /// path; deliberately terminal-free — no colour, no protocol, no
    /// `rune-tui` type appears in `ImageRowRef` itself.
    pub image: Option<ImageRowRef>,
}

/// Which row of a multi-row image a `SnapshotRow` renders, and how many
/// cells wide the renderer should build for it. `row` is 0-based within the
/// image (0 is the anchor row); `width` is the image's own reserved column
/// count, not the pane width — the renderer clips against the pane
/// separately, the same way an ordinary row's spans already do.
/// `target` is `None` for a row synthesized by the whole-document image
/// producer (`image_rows`, plan WP4.S2) — there is exactly one image per
/// document, so `Document::image` alone already answers "which image".
/// `Some(target_text)` for an embed row (`expand_images`, plan WP8/WP9) — a
/// markdown document can carry several embeds at once, so the renderer
/// needs this key to find the RIGHT `EmbedState` (`rune-tui`'s own map,
/// keyed by the same `ImageM::target_text` string) rather than assuming a
/// document has at most one image. Plain `String` data, never a colour, a
/// protocol or a `rune-tui` type — `rune-md` stays terminal-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRowRef {
    pub row: usize,
    pub width: usize,
    pub target: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DisplaySnapshot {
    rows: Vec<SnapshotRow>,
    /// `wrap_to_display[w]` is the display-row index of wrap row `w`'s OWN
    /// (non-synthetic) `SnapshotRow` — sized to `wrap.total_rows()`, one
    /// entry per wrap row, populated only by `expand_tables` (`from_wrap`
    /// alone is already the identity map).
    wrap_to_display: Vec<usize>,
}

impl DisplaySnapshot {
    pub fn from_wrap(wrap: &WrapSnapshot) -> DisplaySnapshot {
        let rows: Vec<SnapshotRow> = wrap
            .segments()
            .iter()
            .enumerate()
            .map(|(i, seg)| SnapshotRow {
                spans: seg.spans.clone(),
                wrap_row: i,
                synthetic: false,
                decor: seg.decor.clone(),
                image: None,
            })
            .collect();
        let wrap_to_display = (0..rows.len()).collect();
        DisplaySnapshot {
            rows,
            wrap_to_display,
        }
    }

    /// Inserts a synthesised border row wherever a table's own
    /// `TableSegInfo::boundary` says one belongs: `┌┬┐` before a
    /// `First`/`Only` row, `└┴┘` after a `Last`/`Only` row, and `├┼┤`
    /// between two consecutive `Body` rows that come from DIFFERENT source
    /// lines (never between two visual rows of the SAME wrapped/pivoted
    /// line — the `model_line` check below is what tells them apart; a
    /// header's own separator line already supplies its own real content
    /// row, so no synthetic border is ever needed between Header and
    /// Separator or Separator and the first Body row).
    ///
    /// A leading (`Top`/`Middle`) border borrows the wrap_row/line-start of
    /// the row it precedes — the one that follows it, which always exists;
    /// a trailing (`Bottom`) border borrows the row it follows — the one
    /// that precedes it, since nothing inside the table follows it. Every
    /// synthetic row's `wrap_row` is therefore always an adjacent content
    /// row's own `wrap_row`.
    pub fn expand_tables(self, wrap: &WrapSnapshot) -> DisplaySnapshot {
        let segments = wrap.segments();
        let mut rows: Vec<SnapshotRow> = Vec::with_capacity(self.rows.len());
        let mut wrap_to_display = vec![0usize; segments.len()];
        // (role, model_line) of the previous row that carried table info —
        // `None` once a non-table row (or the very start) breaks the run.
        let mut prev_table: Option<(TableRole, usize)> = None;

        for row in self.rows {
            let seg = segments.get(row.wrap_row);
            // Filtered on `boxed` at the single source rather than at each
            // border site below: a Pivoted table draws no box at all, and
            // the top, inter-row and bottom borders are three separate
            // decisions that must not be allowed to disagree about it.
            let info = seg.and_then(|s| s.table.as_ref()).filter(|i| i.boxed);
            let model_line = seg.map_or(0, |s| s.model_line);
            let line_start = line_start_of(&row);

            if let Some(info) = info {
                let starts_table = matches!(info.boundary, RowBoundary::First | RowBoundary::Only);
                if starts_table {
                    rows.push(synthetic_border(
                        &info.col_widths,
                        BorderKind::Top,
                        row.wrap_row,
                        line_start,
                    ));
                } else if let Some((prev_role, prev_line)) = prev_table
                    && prev_role == TableRole::Body
                    && info.role == TableRole::Body
                    && prev_line != model_line
                {
                    rows.push(synthetic_border(
                        &info.col_widths,
                        BorderKind::Middle,
                        row.wrap_row,
                        line_start,
                    ));
                }
            }

            if let Some(slot) = wrap_to_display.get_mut(row.wrap_row) {
                *slot = rows.len();
            }
            let ends_table =
                info.is_some_and(|i| matches!(i.boundary, RowBoundary::Last | RowBoundary::Only));
            let widths_for_bottom = info.map(|i| i.col_widths.clone());
            let wrap_row = row.wrap_row;
            rows.push(row);

            if ends_table && let Some(widths) = widths_for_bottom {
                rows.push(synthetic_border(
                    &widths,
                    BorderKind::Bottom,
                    wrap_row,
                    line_start,
                ));
            }

            prev_table = info.map(|i| (i.role, model_line));
        }

        DisplaySnapshot {
            rows,
            wrap_to_display,
        }
    }

    /// Builds a `DisplaySnapshot` of `n` synthetic image rows (plan WP4.S2:
    /// the image producer) — the sole caller is `DocMachine::rebuild`'s
    /// `DocumentKind::Image` branch, whose buffer is always empty (image
    /// bytes never live in a `Buffer`) and so has no wrap rows to derive
    /// real rows from at all. Every row is `synthetic: true`, carries one
    /// `SyntaxSpan::Substituted` with an empty text/range and an empty
    /// `cell_map` (the same "no buffer correspondence" shape `expand_tables`'s
    /// own synthetic border rows use), and an `ImageRowRef` naming its
    /// 0-based row index and the image's reserved column width — the marker
    /// `rune-tui`'s `build_rows` override keys off to build placeholder/
    /// info-card cells instead of the ordinary span-cell path. No row here
    /// corresponds to any wrap row (there is none), so `wrap_to_display`
    /// stays empty and `n` is floored at 1 so an image document is never
    /// left with zero rows to scroll or render into.
    pub fn image_rows(n: usize, width: usize) -> DisplaySnapshot {
        let n = n.max(1);
        let rows = (0..n)
            .map(|row| SnapshotRow {
                spans: vec![SyntaxSpan::substituted(
                    0,
                    String::new(),
                    table_border_scope(),
                    0..0,
                )],
                wrap_row: 0,
                synthetic: true,
                decor: None,
                image: Some(ImageRowRef {
                    row,
                    width,
                    target: None,
                }),
            })
            .collect();
        DisplaySnapshot {
            rows,
            wrap_to_display: Vec::new(),
        }
    }

    pub fn rows(&self) -> &[SnapshotRow] {
        &self.rows
    }

    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    /// The wrap row a display row was built from — for a synthetic row,
    /// the adjacent content row's own wrap row (never its own, since it has
    /// none), per `expand_tables`'s docs.
    pub fn display_to_wrap(&self, row: DisplayRow) -> WrapRow {
        if self.rows.is_empty() {
            return WrapRow(0);
        }
        let row = row.0.min(self.rows.len() - 1);
        WrapRow(self.rows.get(row).map_or(0, |r| r.wrap_row))
    }

    /// The display row a wrap row's OWN content lives at — the inverse of
    /// `display_to_wrap` for a REAL (non-synthetic) row. Every caret/scroll
    /// computation that starts from a wrap-space coordinate (cursors always
    /// do — border rows aren't addressable) must convert through this
    /// before indexing `rows()`.
    pub fn wrap_to_display(&self, row: WrapRow) -> DisplayRow {
        if self.wrap_to_display.is_empty() {
            return DisplayRow(0);
        }
        let row = row.0.min(self.wrap_to_display.len() - 1);
        DisplayRow(self.wrap_to_display.get(row).copied().unwrap_or(0))
    }
}

/// A content row's own spans always tile its whole source line (Gotcha 1),
/// so its first span's range start IS the line's start — the anchor a
/// synthetic border borrows for its own (empty) range.
fn line_start_of(row: &SnapshotRow) -> usize {
    row.spans.first().map_or(0, |s| s.range().start)
}

fn synthetic_border(
    widths: &[usize],
    kind: BorderKind,
    wrap_row: usize,
    line_start: usize,
) -> SnapshotRow {
    let text = border_row(widths, kind);
    let cell_map = vec![None; text.chars().count()];
    let span = SyntaxSpan::substituted_mapped(
        table_border_scope(),
        text,
        line_start..line_start,
        cell_map,
    );
    SnapshotRow {
        spans: vec![span],
        wrap_row,
        synthetic: true,
        decor: None,
        image: None,
    }
}

/// `expand_tables`'s and `expand_images`'s shared round-trip/adjacency
/// invariant (plan WP3.S7, extended by WP8.S1 to cover image rows too):
/// `display_to_wrap(wrap_to_display(r)) == r` for every wrap row, and every
/// synthetic row's `wrap_row` equals an adjacent content row's own. `pub
/// (crate)` (rather than nested inside `mod tests`) so `image_rows`' own
/// tests check image-bearing snapshots against the exact same invariant a
/// table's synthetic borders already are, not a parallel one — only
/// `DisplaySnapshot`'s public API is used, so there is nothing
/// module-private to duplicate.
#[cfg(test)]
#[allow(clippy::indexing_slicing)]
pub(crate) fn assert_round_trip_and_synthetic_adjacency(
    wrap: &WrapSnapshot,
    display: &DisplaySnapshot,
) {
    for w in 0..wrap.total_rows() {
        let d = display.wrap_to_display(WrapRow(w));
        assert_eq!(
            display.display_to_wrap(d),
            WrapRow(w),
            "round trip failed for wrap row {w}"
        );
        assert!(
            !display.rows()[d.0].synthetic,
            "wrap_to_display must land on a real row"
        );
    }

    for (i, row) in display.rows().iter().enumerate() {
        if !row.synthetic {
            continue;
        }
        let prev = i.checked_sub(1).and_then(|j| display.rows().get(j));
        let next = display.rows().get(i + 1);
        assert!(
            prev.is_some_and(|r| r.wrap_row == row.wrap_row)
                || next.is_some_and(|r| r.wrap_row == row.wrap_row),
            "synthetic row {i}'s wrap_row must equal an adjacent content row's"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_syntax::SyntaxLine;
    use rune_syntax::wrap::WrapMap;

    #[test]
    fn display_snapshot_row_count_matches_wrap_in_the_no_table_case() {
        // Two empty lines wrap to exactly 2 rows (each empty `SyntaxLine`
        // gets its own single, empty segment — see `WrapMap::wrap_line`'s
        // `line.spans.is_empty()` case). Pin that concrete count instead of
        // comparing `DisplaySnapshot::from_wrap`'s output back to
        // `wrap.total_rows()` — the very value it copies — which passes
        // trivially by construction regardless of what `total_rows()`
        // actually is.
        let lines = vec![SyntaxLine::default(), SyntaxLine::default()];
        let wrap = WrapMap::new(80).sync("", &lines);
        assert_eq!(wrap.total_rows(), 2);
        let display = DisplaySnapshot::from_wrap(&wrap);
        assert_eq!(display.total_rows(), 2);
        // No table lines: `expand_tables` must be a true no-op.
        let expanded = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
        assert_eq!(expanded.total_rows(), 2);
        for r in expanded.rows() {
            assert!(!r.synthetic);
        }
    }

    /// WP3.S7: a 4-line table (header, separator, two body rows — no
    /// trailing blank line, so wrap has exactly 4 rows, one per source
    /// line) expands to 7 display rows: a synthesised `┌┬┐` before the
    /// header, the header/separator/two body rows unchanged, a synthesised
    /// `├┼┤` between the two body rows (they're different source lines,
    /// both `Body` — the case with no real separator line of their own),
    /// and a synthesised `└┴┘` after the last body row.
    #[test]
    fn four_line_table_expands_to_seven_display_rows() {
        let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        assert_eq!(lines.len(), 4, "fixture must have no trailing blank line");
        let wrap = WrapMap::new(80).sync(content, &lines);
        assert_eq!(wrap.total_rows(), 4);

        let display = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
        assert_eq!(display.total_rows(), 7);

        let synthetic_count = display.rows().iter().filter(|r| r.synthetic).count();
        assert_eq!(synthetic_count, 3, "top + bottom + one inter-row border");

        assert!(display.rows().first().is_some_and(|r| r.synthetic));
        assert!(display.rows().last().is_some_and(|r| r.synthetic));

        // The middle border must sit between the two BODY rows specifically
        // (wrap rows 2 and 3), borrowing the LATTER's wrap_row per a
        // leading border's own convention (see `expand_tables`'s docs) —
        // not between the separator and the first body row, which a
        // `prev_role == Body` check flipped to `!=` would misplace it at
        // instead (same total row/synthetic COUNT either way, so only the
        // exact wrap_row pins the bug).
        let middle = display
            .rows()
            .iter()
            .filter(|r| r.synthetic)
            .nth(1)
            .expect("a middle border between the two body rows");
        assert_eq!(middle.wrap_row, 3);
    }

    /// WP4.S3: the new `image` field must default to `None` for every row
    /// neither the image producer nor `expand_images` touches — both the
    /// real (non-synthetic) rows `from_wrap` builds and the synthetic
    /// table-border rows `expand_tables` inserts.
    #[test]
    fn display_row_image_field_defaults_to_none_for_non_image_rows() {
        let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        let wrap = WrapMap::new(80).sync(content, &lines);
        let display = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
        assert!(display.rows().iter().any(|r| r.synthetic));
        assert!(display.rows().iter().any(|r| !r.synthetic));
        for r in display.rows() {
            assert_eq!(r.image, None, "row should have no image marker");
        }
    }

    /// WP4.S2: `image_rows` reserves exactly `n` rows (floored at 1), every
    /// one synthetic and carrying an `ImageRowRef` naming its own 0-based
    /// row index and the given width.
    #[test]
    fn image_rows_reserves_n_synthetic_rows() {
        let display = DisplaySnapshot::image_rows(4, 12);
        assert_eq!(display.total_rows(), 4);
        for (i, row) in display.rows().iter().enumerate() {
            assert!(row.synthetic);
            assert_eq!(
                row.image,
                Some(ImageRowRef {
                    row: i,
                    width: 12,
                    target: None,
                })
            );
        }
    }

    /// `n` is floored at 1 — an image document must never be left with zero
    /// rows to scroll or render into.
    #[test]
    fn image_rows_floors_at_one() {
        let display = DisplaySnapshot::image_rows(0, 5);
        assert_eq!(display.total_rows(), 1);
    }

    /// `display_to_wrap(wrap_to_display(r)) == r` for every wrap row, and
    /// every synthetic row's `wrap_row` equals an adjacent content row's own
    /// — the round-trip and adjacency assertions.
    #[test]
    fn wrap_display_round_trip_and_synthetic_adjacency() {
        let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        let wrap = WrapMap::new(80).sync(content, &lines);
        let display = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
        assert_round_trip_and_synthetic_adjacency(&wrap, &display);
    }

    fn four_line_table_display() -> DisplaySnapshot {
        let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        let wrap = WrapMap::new(80).sync(content, &lines);
        DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap)
    }

    /// An out-of-range display row clamps to the LAST row (`rows.len() -
    /// 1`), not one past it — pinned with a fixture whose last row's
    /// `wrap_row` (3) differs from every other row's, so a wrong clamp
    /// bound reads a wrong (or out-of-bounds, defaulting to 0) row instead.
    #[test]
    fn display_to_wrap_clamps_to_the_last_row() {
        let display = four_line_table_display();
        assert_eq!(display.total_rows(), 7);
        assert_eq!(display.rows().last().unwrap().wrap_row, 3);
        assert_eq!(display.display_to_wrap(DisplayRow(1000)), WrapRow(3));
    }

    /// Same clamp, the `wrap_to_display` side: an out-of-range wrap row
    /// clamps to `wrap_to_display.len() - 1`, landing on the LAST real
    /// row's own display index (5, the second body row) — not one past it.
    #[test]
    fn wrap_to_display_clamps_to_the_last_wrap_row() {
        let display = four_line_table_display();
        assert_eq!(display.wrap_to_display(WrapRow(1000)), DisplayRow(5));
    }

    /// `line_start_of` reads a content row's own first span's range start
    /// directly — pinned against a value that is neither of the constant
    /// fallbacks (`0`/`1`) a broken producer might return instead.
    #[test]
    fn line_start_of_reads_the_first_spans_range_start() {
        let span = SyntaxSpan::substituted(0, String::new(), table_border_scope(), 42..42);
        let row = SnapshotRow {
            spans: vec![span],
            wrap_row: 0,
            synthetic: false,
            decor: None,
            image: None,
        };
        assert_eq!(line_start_of(&row), 42);
    }
}
