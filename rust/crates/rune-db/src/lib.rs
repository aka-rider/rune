//! `rune-db`: the multiprocess-safe SQLite recovery store over one global
//! WAL database (plan Goal). This crate is `rune-vfs`'s sibling, never its
//! caller: rune-db is "an observer beside the file path, never in it" (plan
//! decision 5) — it journals/snapshots/observes alongside the user's `.md`
//! file, and losing this database must never damage that file.
//!
//! WP2 shipped the skeleton: schema, open ladder, session/liveness
//! identity, the writer/reader thread topology, and the busy/contention
//! retry classifier. WP3 added the durable journal, content-addressed
//! blobs, and recovery snapshots (`append_edit`, `undo_peek`/`redo_peek`/
//! `move_undo_pos`, `create_snapshot`, `recover_document`,
//! `edits_in_range`). WP4 adds observations (`observation.rs`), the
//! conflict-lifecycle comparison (`sync.rs`), `probe`, `materialize` (the
//! CAS write protocol), the Adoption Contract (`adopt.rs`), `load`, and the
//! dead-session reaper (`reaper.rs`). Wiring into `rune-tui` lands in WP5.
//! WP6 (this work package) adds lifecycle: the writer's idle
//! checkpoint/blob-sweep timer and clean-shutdown `TRUNCATE` (`writer.rs`),
//! unreferenced-blob GC (`gc.rs`), old-schema-version file GC
//! (`versioning.rs`), and the multiprocess integration tests
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
mod payload;
mod probe;
mod reader;
mod reaper;
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
pub use materialize::{MatResult, materialize};
pub use observation::{ObsId, Observation, hash_bytes, stat_identity};
pub use probe::probe;
pub use reader::{ReaderHandle, ReaderReply, ReaderRequestKind};
pub use reaper::reap_dead_sessions;
pub use retry::{Classification, classify as classify_retry};
pub use schema::SCHEMA;
pub use session::is_process_alive;
pub use snapshot::{create_snapshot, recover_document};
pub use store::{ClockFn, DEGRADED_WARNING, LivenessCheckFn, Store};
pub use sync::{SyncKind, SyncState, Version, classify_sync, is_dirty, sync};
pub use versioning::{SCHEMA_VERSION, db_file_name, production_db_path};
pub use writer::{DbEvent, OnEvent, OpKind, OpOutcome, WriteOp, WriterHandle};
