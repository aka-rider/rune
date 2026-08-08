//! `EmbedState`/`EmbedSet` (plan WP9): the per-embed lifecycle state a
//! markdown `Document` tracks for every live `![alt](x.png)`/`![[x.png]]`
//! it contains, keyed by `ImageM::target_text` — the same raw string
//! `rune_md::snapshot::ImageDims` (WP8) and `DisplayRow::image`'s own
//! `ImageRowRef::target` (plan WP9) key off, so the renderer, the
//! reconciler and the row-reservation pass never disagree on which embed a
//! key names.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use rune_md::snapshot::ImageDims;

use super::EmbedAllocator;
use crate::graphics::ImageStatus;

/// One tracked embed's own lifecycle state — the `EmbedSet` sibling of a
/// whole image DOCUMENT's `graphics::state::ImageState`, minus the fields
/// an embed has no use for (`bytes_len`, `pending`): a decode failure for
/// one embed never blocks another, so there is no document-wide "needs
/// redecode" flag to carry.
pub struct EmbedState {
    pub abs_path: PathBuf,
    pub id: u32,
    /// The resolved file's mtime at the moment this state was (re)spawned,
    /// or `None` when the stat itself failed — the retry rule's own
    /// sentinel (plan gotcha: "`Failed` is sticky per `(path, mtime)`").
    pub mtime: Option<SystemTime>,
    pub dims: Option<(usize, usize)>,
    pub cells: Option<(usize, usize)>,
    pub decoded: Option<rune_image::decode::Decoded>,
    pub status: ImageStatus,
    pub in_flight: Option<u64>,
}

/// A markdown document's whole embed set (plan WP9.S4/S6): every
/// currently-tracked embed, keyed by its raw target text, plus the id
/// allocator every spawn draws from. Lives on `Document::embeds`; empty and
/// inert for every document kind other than `Markdown` (an image DOCUMENT
/// has no embeds of its own, and every other kind has no images at all).
#[derive(Default)]
pub struct EmbedSet {
    pub alloc: EmbedAllocator,
    pub images: HashMap<String, EmbedState>,
    /// A per-document monotonic counter minted by `schedule_embed_decode`
    /// (plan WP9, mirroring `ImageState::in_flight`'s generation shape) —
    /// unique ACROSS every embed in this document, not just within one, so
    /// `handle_embed_decoded`'s reply can find the right `EmbedState` by
    /// scanning for `in_flight == Some(generation)` without the `Msg`
    /// itself needing to carry a target key (plan constraint: this work
    /// must not grow `runtime/mod.rs`, which already exceeds the 500-line
    /// ceiling from unrelated concurrent work).
    pub(crate) next_generation: u64,
}

impl EmbedSet {
    pub fn new() -> EmbedSet {
        EmbedSet::default()
    }

    /// The per-embed cell footprint map `DocMachine::set_embed_dims` needs
    /// (plan WP8/WP9 wiring) — only embeds whose fit computation has
    /// already run (`cells.is_some()`) are included; an embed still
    /// `Pending` with no footprint yet is absent from the map, which
    /// `expand_images` already treats as "reserve exactly 1 row" (plan
    /// WP8.S4).
    pub fn to_image_dims(&self) -> ImageDims {
        let mut dims = ImageDims::new();
        for (key, state) in &self.images {
            if let Some((cols, rows)) = state.cells {
                dims.insert(key.clone(), cols, rows);
            }
        }
        dims
    }

    /// Whether any tracked embed is wedged mid-decode (`in_flight.is_some()`)
    /// — the one definition `reload_embeds`'s own rescheduling and
    /// `Document::has_reloadable_graphics`'s dispatch gate both read, so a
    /// document whose embeds are all `Live`/`Failed` can never present as
    /// reloadable while the reload itself does nothing.
    pub fn has_wedged(&self) -> bool {
        self.images.values().any(|state| state.in_flight.is_some())
    }
}
