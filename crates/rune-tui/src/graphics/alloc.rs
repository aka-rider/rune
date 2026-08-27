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
