//! `rune-nav`: the producer-agnostic navigation vocabulary shared by every
//! kind of jump-to-target — markdown links today, tree-sitter go-to-
//! definition and imports later. Uses (links, embeds) are graph edges;
//! Defs (headings) are graph nodes. A future headless vault indexer
//! depends on this crate alone, so it must never depend on `rune-md` or
//! `rune-tui`. The vocabulary here is deliberately closed to variants with
//! a live producer: an unconstructed enum variant is a match arm no one
//! can ever prove correct, so a future producer needing a new `UseRole`,
//! `DefRole` or `Anchor` shape adds it (and the resolution logic it needs)
//! in the same change, rather than finding a half-wired one already
//! sitting here.
//!
//! Split (500-line budget) into topic modules: `types` (the data
//! shapes), `resolve` (filesystem resolution), `external` (the URL-scheme
//! allowlist), and `anchor` (anchor-name matching). This file stays the
//! crate's public surface only — declarations and re-exports — so the
//! crate's external API path is unchanged for its consumers.

pub(crate) mod percent;

mod anchor;
mod external;
mod resolve;
mod types;

pub use anchor::anchor_matches;
pub use external::is_external;
pub use resolve::resolve;
pub use rune_syntax::element::ByteRange;
pub use types::{Anchor, AnchorRole, DefRole, Destination, Ref, RefKind, Target, UseRole};
