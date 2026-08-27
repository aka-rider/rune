use super::*;

impl crate::app::App {
    pub fn install_or_join_file_binding(&mut self, db_id: i64, seed_expect_obs: Option<ObsId>) {
        if let std::collections::hash_map::Entry::Vacant(vacant) = self.file_bindings.entry(db_id) {
            vacant.insert(FileBinding::new(seed_expect_obs));
        }
    }

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

    /// Creates `db_id`'s binding (no baseline yet) on the first sighting.
    /// Never races `install_or_join_file_binding`'s own seeding — every
    /// scratch-bind call site passes both a `None` baseline, so whichever
    /// runs first still leaves the other a vacant slot to (not) seed.
    pub(crate) fn set_shared_content(&mut self, db_id: i64, content: &str) {
        self.file_bindings
            .entry(db_id)
            .or_insert_with(|| FileBinding::new(None))
            .shared_content = content.to_string();
    }

    pub fn file_binding_mut(&mut self, db_id: i64) -> Option<&mut FileBinding> {
        self.file_bindings.get_mut(&db_id)
    }

    pub fn doc_db_id(&self, id: DocumentId) -> Option<i64> {
        self.doc(id).and_then(|d| d.doc_db().map(|d| d.db_id))
    }

    pub fn doc_file_binding(&self, id: DocumentId) -> Option<&FileBinding> {
        self.file_binding(self.doc_db_id(id)?)
    }

    pub fn doc_file_binding_mut(&mut self, id: DocumentId) -> Option<&mut FileBinding> {
        let db_id = self.doc_db_id(id)?;
        self.file_binding_mut(db_id)
    }

    pub fn prune_file_binding(&mut self, db_id: i64) {
        let still_referenced = self
            .documents
            .values()
            .any(|d| d.doc_db().is_some_and(|db| db.db_id == db_id));
        if !still_referenced {
            self.file_bindings.remove(&db_id);
        }
    }

    /// Whether any document bound to `db_id` has a save in flight — a probe
    /// against a file another tab is mid-publish to would only read a
    /// soon-to-be-stale disk state.
    pub fn any_save_in_flight_for(&self, db_id: i64) -> bool {
        self.documents
            .values()
            .any(|d| d.doc_db().is_some_and(|db| db.db_id == db_id) && d.save_in_flight())
    }

    /// Every open document bound to `db_id` — used to re-issue a deferred
    /// probe for every tab a save's completion just unblocked, not only the
    /// document whose own save resolved.
    pub fn documents_bound_to(&self, db_id: i64) -> Vec<DocumentId> {
        self.documents
            .iter()
            .filter(|(_, d)| d.doc_db().is_some_and(|db| db.db_id == db_id))
            .map(|(&id, _)| id)
            .collect()
    }
}
