//! `EmbedAllocator` (plan WP9.S6): a persistent map from a Kitty image id to
//! the resolved absolute path that owns it — one instance lives on
//! `Document::embeds` (`EmbedSet`), surviving across reconcile passes so an
//! id stays stable for a path's whole lifetime rather than being
//! re-derived (and potentially colliding differently) every time. Ports
//! Go's `imageIDAllocator` (`golang/pkg/ui/components/markdownedit/
//! image_allocator.go`), minus the clone-on-write shape Go's value-typed
//! `Model` needs — `Document` already owns its `EmbedSet` by `&mut`
//! reference through the normal `App::doc_mut` chokepoint, so plain
//! in-place mutation is both simpler and exactly as safe.

use std::collections::HashMap;

/// The highest 24-bit id `rune_image::alloc_id` (and this allocator's own
/// probe) ever hands out — ids wrap from here back to `1`, never to `0`
/// (`0` is not a valid Kitty image id).
const MAX_ID: u32 = 0x00FF_FFFF;

#[derive(Default)]
pub struct EmbedAllocator {
    by_id: HashMap<u32, String>,
}

impl EmbedAllocator {
    pub fn new() -> EmbedAllocator {
        EmbedAllocator::default()
    }

    /// A deterministic 24-bit id for `key` (an embed's resolved absolute
    /// path, as a string), probing linearly on collision with a DIFFERENT
    /// key and wrapping past [`MAX_ID`] back to `1`. Reallocating the SAME
    /// key (a respawn) returns the id it already holds rather than probing
    /// past it — the mtime-respawn contract requires the base id to stay
    /// unchanged across a respawn (plan gotcha 3).
    pub fn alloc_free_id(&mut self, key: &str) -> u32 {
        let mut id = rune_image::alloc_id(key);
        loop {
            match self.by_id.get(&id) {
                Some(existing) if existing != key => id = probe_next(id),
                _ => {
                    self.by_id.insert(id, key.to_string());
                    return id;
                }
            }
        }
    }

    /// Releases every id this allocator ever handed out for `key` (plan
    /// WP9.S4's despawn) — a later respawn or an unrelated embed can then
    /// reuse them instead of leaking them for the rest of the session.
    pub fn free_all_for(&mut self, key: &str) {
        self.by_id.retain(|_, v| v != key);
    }
}

/// One collision-probe step (plan WP9.S6: "linear probing on collision,
/// wrapping past `0xFFFFFF` back to `1`") — split out so the wrap edge is
/// unit-testable without occupying millions of ids to force a real
/// `alloc_free_id` call all the way to the boundary.
fn probe_next(id: u32) -> u32 {
    if id >= MAX_ID { 1 } else { id + 1 }
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
        let natural = rune_image::alloc_id("/vault/y.png");
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

    #[test]
    fn probing_wraps_past_the_max_id_back_to_one() {
        assert_eq!(probe_next(MAX_ID), 1);
        assert_eq!(probe_next(MAX_ID - 1), MAX_ID);
        assert_eq!(probe_next(1), 2);
    }
}
