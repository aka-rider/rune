//! Terminal-free tree-sitter syntax layer for `rune`. `lang` is the
//! compile-free half — a pure static lookup safe to call from the UI
//! thread. The compiling half (query/parser construction) is added by a
//! later work package on top of this one.

pub mod lang;

pub use lang::resolve;
