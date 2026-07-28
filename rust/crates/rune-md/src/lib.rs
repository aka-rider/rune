//! rune-md: markdown element state machines and the display pipeline
//! (parse -> emit -> wrap -> snapshot). Terminal-free; depends on rune-core,
//! comrak, unicode-width, and the producer-agnostic syntax layer in
//! rune-syntax (`element`'s reveal vocabulary, `SyntaxSpan`/`StyleId`, the
//! wrap pass — WP3).

pub mod element;
pub mod emit;
pub mod parse;
pub mod snapshot;
pub mod wrap;
