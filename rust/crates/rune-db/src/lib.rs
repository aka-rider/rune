//! `rune-db`: the multiprocess-safe SQLite recovery store over one global
//! WAL database (plan Goal). This crate is `rune-vfs`'s sibling, never its
//! caller: rune-db is "an observer beside the file path, never in it" (plan
//! decision 5) — it journals/snapshots/observes alongside the user's `.md`
//! file, and losing this database must never damage that file.
//!
//! WP2 (this work package) ships the skeleton: schema, open ladder,
//! session/liveness identity, the writer/reader thread topology, and the
//! busy/contention retry classifier. Domain verbs (`append_edit`,
//! `materialize`, adoption, the reaper, ...) land in WP3-4; wiring into
//! `rune-tui` lands in WP5; lifecycle (checkpoints, GC, multiprocess tests)
//! lands in WP6.
//!
//! Darwin-only, matching the rest of this workspace (`CLAUDE.md`): no
//! portability shims, no `!darwin` build tags.

mod error;
mod reader;
mod retry;
mod schema;
mod session;
mod store;
mod versioning;
mod writer;

pub use error::Error;
pub use reader::{ReaderHandle, ReaderReply, ReaderRequestKind};
pub use retry::{Classification, classify as classify_retry};
pub use schema::SCHEMA;
pub use session::is_process_alive;
pub use store::{ClockFn, DEGRADED_WARNING, LivenessCheckFn, Store};
pub use versioning::{SCHEMA_VERSION, db_file_name, production_db_path};
pub use writer::{DbEvent, OnEvent, OpKind, WriteOp, WriterHandle};
