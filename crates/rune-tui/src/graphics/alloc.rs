use std::collections::HashMap;

use rune_image::ImageId;

#[derive(Default)]
pub struct TerminalImageAllocator {
    by_id: HashMap<ImageId, String>,
}

impl TerminalImageAllocator {
    pub fn new() -> TerminalImageAllocator {
        TerminalImageAllocator::default()
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
        let mut alloc = TerminalImageAllocator::new();
        let a = alloc.alloc_free_id("/vault/x.png");
        let b = alloc.alloc_free_id("/vault/x.png");
        assert_eq!(a, b);
    }

    #[test]
    fn a_colliding_key_probes_to_a_different_id() {
        let mut alloc = TerminalImageAllocator::new();
        let natural = rune_image::alloc_id(b"/vault/y.png");
        alloc.by_id.insert(natural, "someone-else".to_string());
        let id = alloc.alloc_free_id("/vault/y.png");
        assert_ne!(id, natural);
    }

    #[test]
    fn free_all_for_releases_only_the_named_key() {
        let mut alloc = TerminalImageAllocator::new();
        let a = alloc.alloc_free_id("/vault/x.png");
        let b = alloc.alloc_free_id("/vault/y.png");
        alloc.free_all_for("/vault/x.png");
        assert!(!alloc.by_id.contains_key(&a));
        assert!(alloc.by_id.contains_key(&b));
    }

    #[test]
    fn a_whole_document_key_and_an_embed_key_share_one_namespace() {
        // The whole-document image path and the inline-embed path both key
        // by the resolved absolute path string and must probe through the
        // SAME allocator instance — this is what makes two documents whose
        // paths collide under `rune_image::alloc_id` land on distinct ids
        // regardless of which path opened first.
        let mut alloc = TerminalImageAllocator::new();
        let whole_doc = alloc.alloc_free_id("/vault/shared.png");
        let embed = alloc.alloc_free_id("/vault/shared.png");
        assert_eq!(
            whole_doc, embed,
            "the same path is the same image everywhere"
        );
    }
}
