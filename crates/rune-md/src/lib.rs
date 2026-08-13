//! rune-md: markdown element state machines and the display pipeline
//! (parse -> emit -> wrap -> snapshot). Terminal-free; depends on rune-core,
//! comrak, unicode-width, and the producer-agnostic syntax layer in
//! rune-syntax (`element`'s reveal vocabulary, `SyntaxSpan`, the open scope
//! namespace that replaced `StyleId`, the wrap pass — WP3/WP4). The wrap
//! pass itself lives entirely in `rune_syntax::wrap` now (WP4: the WP3
//! re-export shim `rune_md::wrap` used to carry is gone) — callers import
//! it from there directly.

pub mod catalogue;
pub mod element;
pub mod emit;
pub mod icons;
#[cfg(any(test, feature = "fuzz-hooks"))]
pub mod invariant;
pub mod parse;
pub mod snapshot;
pub mod table;

pub use element::doc::reveal_all;
