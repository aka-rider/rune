//! The element HSM hierarchy: `Block`/`Inline` machine trees. Replaces a
//! flat span list and stateless reveal check with a typed machine tree
//! (plan Context, "The element HSM hierarchy").
//!
//! `doc` holds the root `DocMachine`/`WrapSnapshot`-producing `snapshot`;
//! `block` and `inline` hold the leaf element machines. The shared reveal
//! vocabulary every one of them is built from (`RevealState`, `RevealSm`,
//! `RevealGrant`, `InheritCtx`, `ByteRange`, `CursorProbe`, plus the
//! `WrapState` type `InheritCtx` is typed by) moved to the
//! producer-agnostic `rune-syntax` crate in WP3, so a future tree-sitter
//! producer can emit and consume the same types without depending on this
//! crate. `Block`/`Inline`/`DocMachine` — which stay here, being
//! markdown-specific — import those types from `rune_syntax::element`
//! directly (WP4: the WP3 re-export shim that used to live here is gone).

pub mod block;
pub mod code_region;
pub mod doc;
pub mod inline;
pub mod table;
