//! The wrap pass moved to the producer-agnostic `rune-syntax` crate (WP3:
//! "so a future tree-sitter producer can emit the same types without
//! depending on rune-md"). Re-exported under its historical `rune_md::
//! wrap::*` path — `DocMachine::snapshot`'s own domain description, "parse
//! -> emit -> wrap -> snapshot", still holds; the implementation just lives
//! one crate over now.

pub use rune_syntax::wrap::*;
