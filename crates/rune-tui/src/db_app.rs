use super::*;

impl crate::app::App {
    /// Joins `db_id`'s shared [`FileBinding`], seeding it from
    /// `seed_expect_obs` only if no document has ever bound this `db_id`
    /// before — called exactly once per document, at the moment it installs
    /// its OWN `DocDb` for `db_id` (`db_ack::handle_load_ack`/`handle_
    /// create_scratch_ack`). A SECOND document binding the same `db_id`
    /// finds the entry already present and adopts it as-is: by the writer
    /// thread's own strict FIFO order, this document's fresh `Load` can
    /// never observe a baseline OLDER than what a sibling document's own
    /// earlier save already advanced the shared entry to, so joining rather
    /// than reseeding never regresses it.
    pub fn install_or_join_file_binding(&mut self, db_id: i64, seed_expect_obs: Option<ObsId>) {
        if let std::collections::hash_map::Entry::Vacant(vacant) = self.file_bindings.entry(db_id) {
            vacant.insert(FileBinding::new(seed_expect_obs));
        }
    }

    /// Advances `db_id`'s shared [`FileBinding`] to `obs` unconditionally —
    /// the re-baseline counterpart to [`App::install_or_join_file_binding`],
    /// called only from a `Rebaseline` `Load` ack (`db_ack::
    /// handle_load_ack`). When `db_id` already has a binding, always
    /// overwrites `expect_obs` and clears `pending_rebaseline_hash`, even
    /// though the ordinary join path never would: a re-baseline exists
    /// precisely to correct a baseline that path left stale.
    ///
    /// A missing entry is NOT an inconsistency — the lost-create-race
    /// hand-off (`materialize_ack::reactions`) enqueues a `Rebaseline`
    /// `Load` against the RACER's own row, a `db_id` this process may never
    /// have touched before, so its first-ever sighting legitimately lands
    /// here rather than through [`App::install_or_join_file_binding`]. That
    /// case installs a fresh binding from `obs`, exactly like the ordinary
    /// join path's own first install would.
    pub fn rebaseline_file_binding(&mut self, db_id: i64, obs: ObsId) {
        self.file_bindings
            .entry(db_id)
            .and_modify(|binding| {
                binding.expect_obs = Some(obs);
                binding.pending_rebaseline_hash = None;
            })
            .or_insert_with(|| FileBinding::new(Some(obs)));
    }

    pub fn file_binding(&self, db_id: i64) -> Option<&FileBinding> {
        self.file_bindings.get(&db_id)
    }

    /// Refreshes `db_id`'s shared [`FileBinding::shared_content`] to
    /// `content` — the row's true content as of a bind that just read it —
    /// creating the binding (with no CAS baseline seeded yet) if this is
    /// the very first sighting of `db_id`. Called from
    /// `db_ack::bind_document_row` on every bind, ahead of (or behind, for
    /// a scratch bind) `install_or_join_file_binding`'s own seeding of
    /// `expect_obs` — the two never race in practice: every scratch-bind
    /// call site passes `install_or_join_file_binding` a `None` baseline
    /// too, so whichever runs first, the other finds an already-vacant
    /// baseline to (not) seed either way.
    pub(crate) fn set_shared_content(&mut self, db_id: i64, content: &str) {
        self.file_bindings
            .entry(db_id)
            .or_insert_with(|| FileBinding::new(None))
            .shared_content = content.to_string();
    }

    pub fn file_binding_mut(&mut self, db_id: i64) -> Option<&mut FileBinding> {
        self.file_bindings.get_mut(&db_id)
    }

    /// `id`'s bound `db_id`, or `None` when the document has no store
    /// binding at all — the one place `doc(id).doc_db().map(|d| d.db_id)`
    /// is spelled out, so every caller that only needs the id (never the
    /// [`FileBinding`] itself, e.g. to prune it) shares this instead of
    /// re-deriving it by hand.
    pub fn doc_db_id(&self, id: DocumentId) -> Option<i64> {
        self.doc(id).and_then(|d| d.doc_db().map(|d| d.db_id))
    }

    /// `id`'s shared [`FileBinding`] — `None` when `id` has no store binding
    /// (an untitled/unbound document) or, as an internal-inconsistency
    /// case that should never occur, when it does but no entry was ever
    /// joined for its `db_id`. The one chokepoint for "this document's
    /// store binding, then its shared per-file baseline" — every caller
    /// that used to hand-roll `doc(id).doc_db().map(...).and_then(|db_id|
    /// file_binding(db_id))` shares this instead.
    pub fn doc_file_binding(&self, id: DocumentId) -> Option<&FileBinding> {
        self.file_binding(self.doc_db_id(id)?)
    }

    /// [`Self::doc_file_binding`]'s mutable counterpart.
    pub fn doc_file_binding_mut(&mut self, id: DocumentId) -> Option<&mut FileBinding> {
        let db_id = self.doc_db_id(id)?;
        self.file_binding_mut(db_id)
    }

    /// Removes `db_id`'s shared baseline once no open `Document` references
    /// it any longer — called after every transition that can leave a
    /// `db_id` unreferenced (a close, or a document dropping its own `db`
    /// binding entirely), so a long session opening and closing many files
    /// never grows `file_bindings` unboundedly. A no-op while at least one
    /// document still names `db_id`.
    pub fn prune_file_binding(&mut self, db_id: i64) {
        let still_referenced = self
            .documents
            .values()
            .any(|d| d.doc_db().is_some_and(|db| db.db_id == db_id));
        if !still_referenced {
            self.file_bindings.remove(&db_id);
        }
    }

    /// Whether ANY document currently bound to `db_id` has a save in
    /// flight — the shared-file counterpart to a single document's own
    /// `save_in_flight`: a probe against a file another tab is mid-publish
    /// to would only read a soon-to-be-stale disk state and get dropped by
    /// the epoch check anyway (`db_enqueue::probe`'s own doc comment).
    pub fn any_save_in_flight_for(&self, db_id: i64) -> bool {
        self.documents
            .values()
            .any(|d| d.doc_db().is_some_and(|db| db.db_id == db_id) && d.save_in_flight())
    }

    /// Every currently-open document bound to `db_id` — used to re-issue a
    /// deferred probe for every tab a save's completion just unblocked
    /// (`materialize_ack::handle_materialize_ack`'s tail), not merely the
    /// one document whose OWN save happened to resolve.
    pub fn documents_bound_to(&self, db_id: i64) -> Vec<DocumentId> {
        self.documents
            .iter()
            .filter(|(_, d)| d.doc_db().is_some_and(|db| db.db_id == db_id))
            .map(|(&id, _)| id)
            .collect()
    }
}
