//! Terminal-free tree-sitter syntax layer for `rune`. `lang` and `detect`
//! are the compile-free half — pure static lookups safe to call from the UI
//! thread. `registry` is the compiling half — worker-thread only — and
//! `highlight` is the one function that touches it.

pub mod detect;
pub mod highlight;
pub mod lang;
pub mod registry;

pub use detect::{Detected, detect};
pub use highlight::{HighlightResult, MAX_SPANS, ParsedTree, highlight, highlight_range, parse};
pub use lang::resolve;
pub use registry::registry;
