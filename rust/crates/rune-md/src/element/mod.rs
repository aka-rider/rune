//! The element HSM hierarchy: `Block`/`Inline` machine trees. Replaces Go's
//! flat span list and stateless `shouldReveal`
//! (`pkg/editor/display/span_metadata.go:4-22`) with a typed machine tree
//! (plan Context, "The element HSM hierarchy").
//!
//! `doc` holds the root `DocMachine`/`WrapSnapshot`-producing `snapshot`;
//! `block` and `inline` hold the leaf element machines. The shared reveal
//! vocabulary every one of them is built from (`RevealState`, `RevealSm`,
//! `RevealGrant`, `InheritCtx`, `ByteRange`, `CursorProbe`, plus the
//! `DocState`/`WrapState` types `InheritCtx` is typed by) moved to the
//! producer-agnostic `rune-syntax` crate in WP3, so a future tree-sitter
//! producer can emit and consume the same types without depending on this
//! crate. `Block`/`Inline`/`DocMachine` — which stay here, being
//! markdown-specific — import those types from `rune_syntax::element`
//! directly (WP4: the WP3 re-export shim that used to live here is gone).

pub mod block;
pub mod doc;
pub mod inline;
