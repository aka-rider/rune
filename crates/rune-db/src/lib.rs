//! `rune-db`: the multiprocess-safe SQLite recovery store over one global
//! WAL database. This crate is `rune-vfs`'s sibling, never its caller — a
//! structural fact, not just a convention two modules happen to follow:
//! `materialize.rs` no longer holds a `&dyn Vfs` at all. It is "an
//! observer beside the file path, never in it" — it journals, snapshots,
//! and observes alongside the user's `.md` file, and losing this database
//! must never damage that file. Concretely, the actual `vfs.write_durable`/
//! `exchange` disk publish for a save runs on the CALLER's own thread
//! (`rune-tui`'s save `Cmd`, through its own `Vfs` handle); this crate only
//! hands over the CAS decision data beforehand (`materialize::
//! prepare_materialize`) and records what the caller's disk work concluded
//! afterward (`materialize::record_materialize_outcome`). A writer thread
//! that has died can still fail either bookkeeping step, but it can never
//! again make a save impossible: the publish itself has nothing left to
//! ask this crate for.
//!
//! The pieces: schema/open-ladder/session-liveness identity and the
//! writer/reader thread topology with its busy/contention retry classifier
//! (`schema.rs`/`store.rs`/`session.rs`/`retry.rs`); the durable journal,
//! content-addressed blobs, and recovery snapshots (`journal.rs`/`blob.rs`/
//! `snapshot.rs`); disk-state observations and the conflict-lifecycle
//! comparison (`observation.rs`/`sync.rs`); the CAS write protocol's
//! bookkeeping half and the Adoption Contract (`materialize.rs`/`adopt.rs`);
//! document identity resolution and cross-session crash recovery
//! (`document.rs`/`load.rs`); rename/replace (`rename.rs`); the dead-session
//! reaper and unreferenced-blob/old-schema-version GC (`reaper.rs`/`gc.rs`/
//! `versioning.rs`); and the multiprocess integration tests
//! (`tests/multiprocess.rs`) that exercise all of it against real, separate
//! OS processes.
//!
//! Darwin and Linux: `session.rs` reads process-liveness identity through a
//! platform-gated primitive (`sysctl` on Darwin, `/proc` on Linux) behind
//! the same `proc_started_at`/`boot_time` signatures.

mod adopt;
mod blob;
mod bracket;
mod command_history;
mod confirmation;
mod conn;
mod diag;
mod doc_kind;
mod document;
mod error;
mod gc;
mod ids;
mod inherit;
mod journal;
mod journal_append;
mod lineage;
mod load;
mod load_anchor;
mod materialize;
mod materialize_types;
mod merge_prep;
mod merge_state;
mod obs_origin;
mod observation;
mod open_ladder;
mod paths;
mod payload;
mod probe;
mod reader;
mod reaper;
mod rebind;
mod rename;
mod rename_bind;
mod rename_replace;
mod retry;
mod schema;
mod scratch;
mod search_history;
mod session;
mod snapshot;
mod store;
mod store_ops;
mod sync;
#[cfg(test)]
mod test_support;
mod versioning;
mod writer;
mod writer_exec;
mod writer_lifecycle;
mod writer_ops;

pub use bracket::stat_facts_from;
#[cfg(feature = "test-support")]
pub use confirmation::Confirmation;
#[cfg(feature = "test-support")]
pub use conn::{
    fresh_memory_uri_for_test, open_raw_connection_at_path_for_test,
    open_recovery_store_at_path_for_test, open_recovery_store_in_memory_for_test,
};
pub use error::Error;
pub use ids::{BlobHash, DocId, ObsId, Seq, SessionId};
#[cfg(feature = "test-support")]
pub use journal::{append_edit, current_seq, move_undo_pos, redo_peek, undo_peek};
pub use load::LoadResult;
pub use materialize::{MatResult, MaterializeOutcome, MaterializePrep, MaterializeTarget};
pub use merge_prep::{MergePrepOutcome, MergePrepResult};
pub use merge_state::MergeCloseState;
pub use obs_origin::ObsOrigin;
pub use observation::{Observation, StatFacts, hash_bytes};
pub use reader::{ReaderQuery, ReaderReply, ReaderRequestKind};
#[cfg(feature = "test-support")]
pub use reaper::reap_dead_sessions;
pub use rename::RenameOutcome;
#[cfg(feature = "test-support")]
pub use session::is_process_alive;
#[cfg(feature = "test-support")]
pub use snapshot::recover_document;
pub use store::{ClockFn, DEGRADED_WARNING, Store};
pub use sync::{SyncKind, SyncState, Version};
pub use versioning::{SCHEMA_VERSION, db_file_name};
pub use writer::{DbEvent, OnEvent, OpOutcome, QUEUE_DEPTH};
