//! Inline embed lifecycle (plan WP9): a markdown document can carry several
//! `![alt](x.png)`/`![[x.png]]` images at once, unlike an image DOCUMENT
//! (`graphics::state::ImageState`, exactly one per `Document`) — so this is
//! a whole per-document SET, keyed by `ImageM::target_text` (the same key
//! `rune_md::snapshot::ImageDims` uses), each entry independently allocated,
//! decoded, transmitted and torn down.
//!
//! Split for the §1.6 budget: [`alloc`] holds the id allocator,
//! [`state`] holds `EmbedState`/`EmbedSet`, [`reconcile`] holds the
//! spawn/despawn pass (`sync_embeds`), and [`decode`] holds the decode
//! `Cmd`/reply handler pair.

mod alloc;
mod decode;
mod reconcile;
mod state;

pub use alloc::EmbedAllocator;
pub use state::{EmbedSet, EmbedState};

pub(crate) use decode::handle_embed_decoded;
pub(crate) use reconcile::{reload_embeds, sync_embeds};
