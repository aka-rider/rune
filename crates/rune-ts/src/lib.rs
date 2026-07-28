//! Terminal-free tree-sitter syntax layer for `rune`. `lang` is the
//! compile-free half — a pure static lookup safe to call from the UI
//! thread. `registry` is the compiling half — worker-thread only — and
//! `highlight` is the one function that touches it.

pub mod highlight;
pub mod lang;
pub mod registry;

pub use highlight::{HighlightResult, MAX_SPANS, highlight};
pub use lang::resolve;
pub use registry::registry;
