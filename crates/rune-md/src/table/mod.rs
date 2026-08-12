//! Table rendering (WP2.S5 onward): span-tiling, cell rendering, and Grid/
//! Wrapped/Pivoted layout for GFM tables. `tiling` holds the row-tiling
//! chokepoint (`row_spans`/`extra_row_spans`); see its own docs.

pub mod layout;
pub mod pivot;
pub mod render;
mod tiling;
pub mod wrapped;

pub use tiling::{extra_row_spans, row_spans};

use rune_syntax::ScopeId;

/// One visible char's provenance inside a table's rendered row: the
/// absolute buffer offset it maps back to, or `None` for a decorative char
/// with no buffer correspondence at all (a `│` border, a padding space, a
/// pivot label borrowed from a different line — see `table::layout`'s
/// docs). `scope` is carried alongside so a caller building a flat
/// char-by-char sequence (`layout::grid_row`/`separator_row`) can group
/// contiguous same-scope runs before handing them to `row_spans` — `scope`
/// itself is never read by `row_spans`, which only ever consumes a run's
/// OWN uniform `ScopeId` (the third element of its `runs` tuples) and each
/// char's `buf` (to build `cell_map`).
#[derive(Clone, Copy, Debug)]
pub struct CellSrc {
    pub buf: Option<u32>,
    pub scope: ScopeId,
}
