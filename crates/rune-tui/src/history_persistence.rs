//! One debounced write-behind shape, shared by the search bar's query
//! history and the command palette's command-name history: both track
//! "the last value we already persisted" (so re-landing on it or repeating
//! Enter enqueues nothing) and the writer-FIFO op id of any write still in
//! flight (so `db_dispatch::handle_db_event` can recognize a later ack for
//! it and retire it, or a later `Err` and roll the debounce back rather
//! than sticky-degrading the whole store over a cosmetic write).

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

    /// Debounces `value` against the last value this persisted, then hands
    /// it to `write` (the caller's own recovery-store call) unless `db` is
    /// absent/degraded. `None` means nothing was enqueued (debounced, no
    /// store, or degraded); `Some` carries `write`'s own result — the
    /// caller reports a failure through the message log itself, since only
    /// it knows the right noun ("search history"/"command history").
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

    /// Retires `op_id` on a successful ack — a no-op if it isn't (or is no
    /// longer) this persistence's own in-flight op.
    pub fn ack(&mut self, op_id: u64) -> bool {
        self.ops.remove(&op_id)
    }

    /// Retires `op_id` on a failed ack and rolls the debounce back, so the
    /// same value is eligible to retry on its next use instead of being
    /// mistaken for already-durable — a no-op (returns `false`) if it isn't
    /// this persistence's own in-flight op.
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
