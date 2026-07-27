//! `rune-db`: the multiprocess-safe SQLite recovery store over one global
//! WAL database (plan Goal). This crate is `rune-vfs`'s sibling, never its
//! caller: rune-db is "an observer beside the file path, never in it" (plan
//! decision 5) — it journals/snapshots/observes alongside the user's `.md`
//! file, and losing this database must never damage that file.
//!
//! WP2 shipped the skeleton: schema, open ladder, session/liveness
//! identity, the writer/reader thread topology, and the busy/contention
//! retry classifier. WP3 (this work package) adds the durable journal,
//! content-addressed blobs, and recovery snapshots (`append_edit`,
//! `undo_peek`/`redo_peek`/`move_undo_pos`, `create_snapshot`,
//! `recover_document`, `edits_in_range`). Observations/probe/materialize/
//! adoption/reaper land in WP4; wiring into `rune-tui` lands in WP5;
//! lifecycle (checkpoints, GC, multiprocess tests) lands in WP6.
//!
//! Darwin-only, matching the rest of this workspace (`CLAUDE.md`): no
//! portability shims, no `!darwin` build tags.

mod blob;
mod error;
mod journal;
mod payload;
mod reader;
mod retry;
mod schema;
mod session;
mod snapshot;
mod store;
mod versioning;
mod writer;

pub use error::Error;
pub use journal::{
    EditRow, Step, append_edit, current_seq, edits_in_range, move_undo_pos, redo_peek, undo_peek,
};
pub use reader::{ReaderHandle, ReaderReply, ReaderRequestKind};
pub use retry::{Classification, classify as classify_retry};
pub use schema::SCHEMA;
pub use session::is_process_alive;
pub use snapshot::{create_snapshot, recover_document};
pub use store::{ClockFn, DEGRADED_WARNING, LivenessCheckFn, Store};
pub use versioning::{SCHEMA_VERSION, db_file_name, production_db_path};
pub use writer::{DbEvent, OnEvent, OpKind, WriteOp, WriterHandle};
