//! `DocumentMap`: `App::documents`'s backing collection, split out of
//! `document.rs` per §1.6 — a self-contained collection type whose only
//! dependency on `document.rs` is the `Document`/`DocumentId` types
//! themselves.

use std::collections::BTreeMap;

use crate::document::{Document, DocumentId};

/// `App::documents` — never empty, by CONSTRUCTION rather than by every
/// caller remembering a floor check (review fix, [rune-tui A 6]: this used
/// to be a plain `BTreeMap` plus a doc comment asserting "nothing removes an
/// entry", which went false the moment `workspace::close_now` shipped,
/// leaving `App::active_doc`/`active_doc_mut` reaching for `#[allow(clippy::
/// unwrap_used)]` to paper over the fallback branch it then needed).
/// `anchor` holds one guaranteed-present `(DocumentId, Document)` pair
/// outside the `BTreeMap`, so "at least one entry exists" is a fact about
/// this type's fields, not a runtime invariant something else could violate
/// — [`DocumentMap::get_or_anchor`]/[`get_or_anchor_mut`] can therefore
/// answer "the active document, or SOME live document" with a real
/// reference, never a panic or an `#[allow]`.
///
/// `App::mint_doc_id` only ever hands out increasing ids, so every id ever
/// inserted into `rest` is greater than `anchor.0` at the moment it's
/// inserted, and removing `anchor` promotes `rest`'s lowest-keyed entry —
/// itself still lower than everything left in `rest` — to take its place.
/// `anchor.0` therefore stays the running minimum for the type's entire
/// lifetime, which is what lets `keys`/`values`/`iter` below just chain
/// `anchor` in front of `rest`'s already-sorted iteration instead of
/// merging.
pub struct DocumentMap {
    anchor: (DocumentId, Document),
    rest: BTreeMap<DocumentId, Document>,
}

impl DocumentMap {
    pub fn new(id: DocumentId, doc: Document) -> DocumentMap {
        DocumentMap {
            anchor: (id, doc),
            rest: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// Never `true` — the type's entire reason to exist — but spelled out
    /// so this doesn't trip clippy's `len_without_is_empty`.
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

    /// `id`, if live, else the anchor — always a real document, never a
    /// panic. The chokepoint `App::active_doc` reads through: `id` not
    /// (or no longer) naming a live entry is exactly the "shouldn't happen,
    /// but `active` future callers must reassign before removing" case this
    /// type exists to make survivable rather than merely documented.
    pub fn get_or_anchor(&self, id: &DocumentId) -> &Document {
        self.get(id).unwrap_or(&self.anchor.1)
    }

    /// The `get_mut` counterpart of [`get_or_anchor`]. Written against
    /// `anchor`/`rest` directly, not `self.get_mut` + a fallback borrow —
    /// two overlapping `&mut self` borrows from one match don't survive the
    /// borrow checker cleanly, an early return does.
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
        if id == self.anchor.0 {
            Some(std::mem::replace(&mut self.anchor.1, doc))
        } else {
            self.rest.insert(id, doc)
        }
    }

    /// Removes `id`, refusing (returning `None`, leaving `self` unchanged)
    /// when `id` is the anchor and `rest` is empty — the non-emptiness floor
    /// `workspace::close_now`/`request_close` already check before calling
    /// this, kept here too as this type's own structural guarantee rather
    /// than trusting every future caller to remember it independently.
    pub fn remove(&mut self, id: &DocumentId) -> Option<Document> {
        if *id != self.anchor.0 {
            return self.rest.remove(id);
        }
        let (&next_id, _) = self.rest.iter().next()?;
        let next_doc = self.rest.remove(&next_id)?;
        Some(std::mem::replace(&mut self.anchor, (next_id, next_doc)).1)
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

    /// Review fix (plan WP5.S5): removing the id `DocumentMap`'s internal
    /// anchor currently holds must promote a survivor rather than ever
    /// leaving the map able to answer "empty" — `remove` on the LAST
    /// document is refused outright.
    #[test]
    fn document_map_promotes_a_survivor_when_the_anchor_entry_is_removed() {
        let mut app = crate::app::App::new(
            Buffer::new("a"),
            None,
            std::sync::Arc::new(Mem::new()),
            None,
        );
        let a = app.active; // the anchor: the lowest-minted id
        let b = app.open_document(Buffer::new("b"));

        assert_eq!(app.documents.len(), 2);
        assert!(app.documents.remove(&a).is_some());
        assert_eq!(app.documents.len(), 1);
        assert!(app.documents.get(&a).is_none());
        assert!(app.documents.get(&b).is_some());

        // The map is never empty, by construction: removing its one
        // remaining entry is refused rather than producing an empty map.
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

        // `b` no longer names a live document — `get_or_anchor` must still
        // return a REAL document (the anchor, `a`) rather than panicking.
        let fallback = app.documents.get_or_anchor(&b);
        assert_eq!(fallback.buffer.content(), "a");
        let _ = a;
    }
}
