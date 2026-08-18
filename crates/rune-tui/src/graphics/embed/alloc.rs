//! `EmbedAllocator`: a persistent map from a Kitty image id to
//! the resolved absolute path that owns it — one instance lives on
//! `Document::embeds` (`EmbedSet`), surviving across reconcile passes so an
//! id stays stable for a path's whole lifetime rather than being
//! re-derived (and potentially colliding differently) every time. Owned
//! directly, with no clone-on-write layer — `Document` already owns its
//! `EmbedSet` by `&mut` reference through the normal `App::doc_mut`
//! chokepoint, so plain in-place mutation is both simpler and exactly as
//! safe.

use std::collections::HashMap;

use rune_image::ImageId;

#[derive(Default)]
pub struct EmbedAllocator {
    by_id: HashMap<ImageId, String>,
}

impl EmbedAllocator {
    pub fn new() -> EmbedAllocator {
        EmbedAllocator::default()
    }

    /// A deterministic 24-bit id for `key` (an embed's resolved absolute
    /// path, as a string), probing linearly on collision with a DIFFERENT
    /// key and wrapping past [`rune_image::ids::MAX_ID`] back to `1` (via
    /// [`ImageId::next`], the same collision-probe step every allocation
    /// scheme built over `rune_image`'s ids shares). Reallocating the SAME
    /// key (a respawn) returns the id it already holds rather than probing
    /// past it — a respawn triggered by an mtime change must keep the same
    /// base id, or the Kitty terminal treats it as a brand-new image and
    /// leaks the old placement.
    pub fn alloc_free_id(&mut self, key: &str) -> ImageId {
        let mut id = rune_image::alloc_id(key.as_bytes());
        loop {
            match self.by_id.get(&id) {
                Some(existing) if existing != key => id = id.next(),
                _ => {
                    self.by_id.insert(id, key.to_string());
                    return id;
                }
            }
        }
    }

    /// Releases every id this allocator ever handed out for `key` — a later
    /// respawn or an unrelated embed can then reuse them instead of leaking
    /// them for the rest of the session.
    pub fn free_all_for(&mut self, key: &str) {
        self.by_id.retain(|_, v| v != key);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_same_key_always_gets_the_same_id_back() {
        let mut alloc = EmbedAllocator::new();
        let a = alloc.alloc_free_id("/vault/x.png");
        let b = alloc.alloc_free_id("/vault/x.png");
        assert_eq!(a, b);
    }

    #[test]
    fn a_colliding_key_probes_to_a_different_id() {
        let mut alloc = EmbedAllocator::new();
        // Force a collision: seed the natural id of "/vault/y.png" under a
        // different key first, so "/vault/y.png" itself must probe past it.
        let natural = rune_image::alloc_id(b"/vault/y.png");
        alloc.by_id.insert(natural, "someone-else".to_string());
        let id = alloc.alloc_free_id("/vault/y.png");
        assert_ne!(id, natural);
    }

    #[test]
    fn free_all_for_releases_only_the_named_key() {
        let mut alloc = EmbedAllocator::new();
        let a = alloc.alloc_free_id("/vault/x.png");
        let b = alloc.alloc_free_id("/vault/y.png");
        alloc.free_all_for("/vault/x.png");
        assert!(!alloc.by_id.contains_key(&a));
        assert!(alloc.by_id.contains_key(&b));
    }
}
