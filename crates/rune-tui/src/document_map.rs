use std::collections::BTreeMap;

use crate::document::{Document, DocumentId};
use crate::resolved::ResolvedPath;

pub struct PathRekey(());

pub struct DocumentMap {
    anchor: (DocumentId, Document),
    rest: BTreeMap<DocumentId, Document>,
    order: Vec<DocumentId>,
    mru: Vec<DocumentId>,
    by_path: BTreeMap<ResolvedPath, DocumentId>,
}

impl DocumentMap {
    pub fn new(id: DocumentId, doc: Document) -> DocumentMap {
        let by_path = doc
            .resolved_path()
            .map(|path| BTreeMap::from([(path.clone(), id)]))
            .unwrap_or_default();
        DocumentMap {
            anchor: (id, doc),
            rest: BTreeMap::new(),
            order: vec![id],
            mru: vec![id],
            by_path,
        }
    }

    pub fn document_for(&self, path: &ResolvedPath) -> Option<DocumentId> {
        self.by_path.get(path).copied()
    }

    pub fn rebind(&mut self, id: DocumentId, path: ResolvedPath) {
        if self.get(&id).is_none() {
            return;
        }
        self.unindex(id);
        self.dispossess_claimants(&path, id);
        self.by_path.insert(path.clone(), id);
        if let Some(doc) = self.get_mut(&id) {
            doc.rebind_path(path, PathRekey(()));
        }
        self.reindex_claimants();
    }

    fn dispossess_claimants(&mut self, path: &ResolvedPath, winner: DocumentId) {
        let losers: Vec<DocumentId> = self
            .iter()
            .filter(|(id, doc)| **id != winner && doc.resolved_path() == Some(path))
            .map(|(id, _)| *id)
            .collect();
        for loser in losers {
            self.unindex(loser);
            if let Some(doc) = self.get_mut(&loser) {
                doc.unbind_path(PathRekey(()));
            }
        }
    }

    fn index(&mut self, id: DocumentId) {
        if let Some(path) = self.get(&id).and_then(Document::resolved_path).cloned() {
            self.by_path.insert(path, id);
        }
    }

    fn unindex(&mut self, id: DocumentId) {
        self.by_path.retain(|_, holder| *holder != id);
    }

    fn reindex_claimants(&mut self) {
        let claims: Vec<(ResolvedPath, DocumentId)> = self
            .iter()
            .filter_map(|(id, doc)| doc.resolved_path().map(|path| (path.clone(), *id)))
            .collect();
        for (path, id) in claims {
            self.by_path.entry(path).or_insert(id);
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
        } else {
            self.unindex(id);
        }
        self.index(id);
        self.reindex_claimants();
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
        self.unindex(*id);
        self.reindex_claimants();
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
    use std::path::Path;

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
    fn a_promoted_survivor_still_answers_to_its_own_path() {
        let vfs: std::sync::Arc<dyn rune_vfs::Vfs + Send + Sync> = std::sync::Arc::new(Mem::new());
        let a_path = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new("/a.md"))
            .expect("Mem resolves any spelling");
        let b_path = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new("/b.md"))
            .expect("Mem resolves any spelling");
        let mut app = crate::app::App::new(Buffer::new("a"), Some(a_path.clone()), vfs, None);
        let a = app.active;
        let b = app.open_document_bound(Buffer::new("b"), b_path.clone());

        assert!(app.documents.remove(&a).is_some());

        assert_eq!(app.documents.document_for(&a_path), None);
        assert_eq!(
            app.documents.document_for(&b_path),
            Some(b),
            "promotion into the anchor slot must not lose the survivor's path"
        );
    }

    #[test]
    fn rebinding_onto_a_held_path_unbinds_the_document_that_held_it() {
        let vfs: std::sync::Arc<dyn rune_vfs::Vfs + Send + Sync> = std::sync::Arc::new(Mem::new());
        let a_path = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new("/a.md"))
            .expect("Mem resolves any spelling");
        let b_path = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new("/b.md"))
            .expect("Mem resolves any spelling");
        let mut app = crate::app::App::new(Buffer::new("a"), Some(a_path.clone()), vfs, None);
        let a = app.active;
        let b = app.open_document_bound(Buffer::new("b"), b_path.clone());

        app.documents.rebind(b, a_path.clone());

        let loser = app.documents.get(&a).expect("the loser stays open");
        assert_eq!(
            loser.resolved_path(),
            None,
            "a document whose file was taken over must stop answering to it"
        );
        assert_eq!(loser.buffer.content(), "a", "the loser keeps its words");
        assert_eq!(loser.file_name(), "a.md", "the tab keeps the old file name");
        assert_eq!(app.documents.document_for(&a_path), Some(b));
        assert_eq!(app.documents.document_for(&b_path), None);
    }

    #[test]
    fn closing_the_indexed_tab_of_a_shared_file_leaves_the_other_tab_answering_for_it() {
        let vfs: std::sync::Arc<dyn rune_vfs::Vfs + Send + Sync> = std::sync::Arc::new(Mem::new());
        let shared = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new("/shared.md"))
            .expect("Mem resolves any spelling");
        let mut app = crate::app::App::new(Buffer::new("a"), Some(shared.clone()), vfs, None);
        let a = app.active;
        let b = app.open_document_bound(Buffer::new("b"), shared.clone());
        assert_eq!(
            app.documents.document_for(&shared),
            Some(b),
            "test setup: two tabs may share one file, and the newest is indexed"
        );

        app.documents.remove(&b).expect("b closes");

        assert_eq!(
            app.documents.document_for(&shared),
            Some(a),
            "the tab still open on the file must answer for it"
        );
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
