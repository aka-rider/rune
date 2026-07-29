//! `rune-db`: the multiprocess-safe SQLite recovery store over one global
//! WAL database. This crate is `rune-vfs`'s sibling, never its caller — a
//! structural fact, not just a convention two modules happen to follow
//! (WP7): `materialize.rs` no longer holds a `&dyn Vfs` at all. It is "an
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
//! Darwin-only, matching the rest of this workspace (`CLAUDE.md`): no
//! portability shims, no `!darwin` build tags.

mod adopt;
mod blob;
mod document;
mod error;
mod gc;
mod journal;
mod load;
mod materialize;
mod observation;
mod paths;
mod payload;
mod probe;
mod reader;
mod reaper;
mod rename;
mod retry;
mod schema;
mod session;
mod snapshot;
mod store;
mod sync;
mod versioning;
mod writer;

pub use adopt::{adopt_equal, resolve_abandon, resolve_adopt};
pub use document::{DocRef, open_path};
pub use error::Error;
pub use journal::{
    EditRow, Step, append_edit, current_seq, edits_in_range, move_undo_pos, redo_peek, undo_peek,
};
pub use load::{LoadResult, has_history, load};
pub use materialize::{
    DocSession, MatResult, MaterializeOutcome, MaterializePrep, prepare_materialize,
    record_materialize_outcome,
};
pub use observation::{ObsId, Observation, ObservationMeta, StatFacts, hash_bytes, stat_identity};
pub use probe::probe;
pub use reader::{ReaderHandle, ReaderReply, ReaderRequestKind};
pub use reaper::reap_dead_sessions;
pub use rename::{RenameOutcome, rename_bind, rename_replace};
pub use retry::{Classification, classify as classify_retry};
pub use schema::SCHEMA;
pub use session::is_process_alive;
pub use snapshot::{create_snapshot, recover_document};
pub use store::{ClockFn, DEGRADED_WARNING, LivenessCheckFn, Store};
pub use sync::{SyncKind, SyncState, Version, classify_sync, is_dirty, sync};
pub use versioning::{SCHEMA_VERSION, db_file_name, production_db_path};
pub use writer::{DbEvent, OnEvent, OpKind, OpOutcome, WriteOp, WriterHandle};
