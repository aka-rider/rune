use std::collections::BTreeMap;

use crate::document::{Document, DocumentId};

pub struct DocumentMap {
    anchor: (DocumentId, Document),
    rest: BTreeMap<DocumentId, Document>,
    order: Vec<DocumentId>,
    mru: Vec<DocumentId>,
}

impl DocumentMap {
    pub fn new(id: DocumentId, doc: Document) -> DocumentMap {
        DocumentMap {
            anchor: (id, doc),
            rest: BTreeMap::new(),
            order: vec![id],
            mru: vec![id],
        }
    }

    pub fn order(&self) -> &[DocumentId] {
        &self.order
    }

    pub fn mru(&self) -> &[DocumentId] {
        &self.mru
    }

    pub fn touch(&mut self, id: DocumentId) {
        self.mru.retain(|&t| t != id);
        self.mru.push(id);
    }

    pub fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// Always `false` — the type is never empty by construction; spelled
    /// out only to satisfy clippy's `len_without_is_empty`.
    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn contains_key(&self, id: &DocumentId) -> bool {
        *id == self.anchor.0 || self.rest.contains_key(id)
    }

    pub fn get(&self, id: &DocumentId) -> Option<&Document> {
        if *id == self.anchor.0 {
            Some(&self.anchor.1)
        } else {
            self.rest.get(id)
        }
    }

    pub fn get_mut(&mut self, id: &DocumentId) -> Option<&mut Document> {
        if *id == self.anchor.0 {
            Some(&mut self.anchor.1)
        } else {
            self.rest.get_mut(id)
        }
    }

    pub fn get_or_anchor(&self, id: &DocumentId) -> &Document {
        self.get(id).unwrap_or(&self.anchor.1)
    }

    // An early return, not `self.get_mut(id).unwrap_or(&mut self.anchor.1)`:
    // that shape needs two overlapping `&mut self` borrows alive from one
    // match, which the borrow checker rejects.
    pub fn get_or_anchor_mut(&mut self, id: &DocumentId) -> &mut Document {
        if *id == self.anchor.0 {
            return &mut self.anchor.1;
        }
        match self.rest.get_mut(id) {
            Some(doc) => doc,
            None => &mut self.anchor.1,
        }
    }

    pub fn insert(&mut self, id: DocumentId, doc: Document) -> Option<Document> {
        let previous = if id == self.anchor.0 {
            Some(std::mem::replace(&mut self.anchor.1, doc))
        } else {
            self.rest.insert(id, doc)
        };
        if previous.is_none() {
            self.order.push(id);
            self.mru.push(id);
        }
        previous
    }

    pub fn remove(&mut self, id: &DocumentId) -> Option<Document> {
        let removed = if *id != self.anchor.0 {
            self.rest.remove(id)?
        } else {
            let (&next_id, _) = self.rest.iter().next()?;
            let next_doc = self.rest.remove(&next_id)?;
            std::mem::replace(&mut self.anchor, (next_id, next_doc)).1
        };
        self.order.retain(|t| t != id);
        self.mru.retain(|t| t != id);
        Some(removed)
    }

    pub fn keys(&self) -> impl Iterator<Item = &DocumentId> {
        std::iter::once(&self.anchor.0).chain(self.rest.keys())
    }

    pub fn values(&self) -> impl Iterator<Item = &Document> {
        std::iter::once(&self.anchor.1).chain(self.rest.values())
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Document> {
        std::iter::once(&mut self.anchor.1).chain(self.rest.values_mut())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&DocumentId, &Document)> {
        std::iter::once((&self.anchor.0, &self.anchor.1)).chain(self.rest.iter())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;

    #[test]
    fn document_map_promotes_a_survivor_when_the_anchor_entry_is_removed() {
        let mut app = crate::app::App::new(
            Buffer::new("a"),
            None,
            std::sync::Arc::new(Mem::new()),
            None,
        );
        let a = app.active;
        let b = app.open_document(Buffer::new("b"));

        assert_eq!(app.documents.len(), 2);
        assert!(app.documents.remove(&a).is_some());
        assert_eq!(app.documents.len(), 1);
        assert!(app.documents.get(&a).is_none());
        assert!(app.documents.get(&b).is_some());

        assert!(app.documents.remove(&b).is_none());
        assert_eq!(app.documents.len(), 1);
        assert!(app.documents.get(&b).is_some());
    }

    #[test]
    fn document_map_get_or_anchor_falls_back_to_a_real_document_for_a_stale_id() {
        let mut app = crate::app::App::new(
            Buffer::new("a"),
            None,
            std::sync::Arc::new(Mem::new()),
            None,
        );
        let a = app.active;
        let b = app.open_document(Buffer::new("b"));
        app.documents.remove(&b).expect("b removed");

        let fallback = app.documents.get_or_anchor(&b);
        assert_eq!(fallback.buffer.content(), "a");
        let _ = a;
    }
}
