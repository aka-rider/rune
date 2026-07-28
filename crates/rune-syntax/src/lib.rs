//! rune-syntax: the producer-agnostic syntax layer (WP3) — the reveal
//! state-machine vocabulary (`element`), the `SyntaxSpan`/`SyntaxLine`/
//! `SyntaxSnapshot` coordinate model (`syntax`), the open scope namespace
//! that replaced the closed `StyleId` enum (`scope`, WP4), and the wrap pass
//! (`wrap`). `rune-md`'s comrak-driven emitter is the only producer today; a
//! future tree-sitter producer (`rune-ts`) emits the same types without
//! depending on `rune-md`. Terminal-free; depends only on rune-core,
//! unicode-width and unicode-segmentation.

pub mod element;
pub mod kind;
pub mod scope;
pub mod syntax;
pub mod wrap;

pub use kind::DocumentKind;
pub use scope::{ScopeId, ScopeTable};
pub use syntax::{CellMap, SyntaxLine, SyntaxSnapshot, SyntaxSpan, merge_overlapping};
