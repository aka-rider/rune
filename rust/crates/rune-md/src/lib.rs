//! rune-md: markdown element state machines and the display pipeline
//! (parse -> emit -> wrap -> snapshot). Terminal-free; depends only on
//! rune-core, comrak, unicode-width, and unicode-segmentation.

pub mod element;
pub mod emit;
pub mod parse;
