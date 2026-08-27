use std::collections::HashSet;

use crate::db::Db;

#[derive(Default)]
pub struct HistoryPersistence {
    pub(crate) last_persisted: Option<String>,
    pub(crate) ops: HashSet<u64>,
}

impl HistoryPersistence {
    pub fn new() -> HistoryPersistence {
        HistoryPersistence::default()
    }

    /// `None` means nothing was enqueued (debounced, no store, or
    /// degraded); `Some` carries `write`'s own result — the caller surfaces
    /// a failure itself, since only it knows the right noun ("search
    /// history" vs "command history").
    pub fn touch(
        &mut self,
        db: Option<&Db>,
        value: &str,
        write: impl FnOnce(&Db) -> Result<u64, rune_db::Error>,
    ) -> Option<Result<u64, rune_db::Error>> {
        if self.last_persisted.as_deref() == Some(value) {
            return None;
        }
        let db = db?;
        if db.degraded {
            return None;
        }
        match write(db) {
            Ok(op_id) => {
                self.ops.insert(op_id);
                self.last_persisted = Some(value.to_string());
                Some(Ok(op_id))
            }
            Err(e) => Some(Err(e)),
        }
    }

    pub fn ack(&mut self, op_id: u64) -> bool {
        self.ops.remove(&op_id)
    }

    /// Rolls the debounce back on a failed write, so the same value is
    /// eligible to retry rather than being mistaken for already durable.
    pub fn fail(&mut self, op_id: u64) -> bool {
        if self.ops.remove(&op_id) {
            self.last_persisted = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_store_never_calls_write() {
        let mut hp = HistoryPersistence::new();
        let called = std::cell::Cell::new(false);
        assert!(
            hp.touch(None, "hi", |_| {
                called.set(true);
                Ok(1)
            })
            .is_none()
        );
        assert!(!called.get());
    }

    #[test]
    fn an_untracked_op_id_is_not_acked_or_failed() {
        let mut hp = HistoryPersistence::new();
        assert!(!hp.fail(1));
        assert!(!hp.ack(1));
    }
}
