use std::path::Path;

use rusqlite::Connection;

use crate::Error;
use crate::conn::{self, RecoveryTarget};
use crate::store::DEGRADED_WARNING;

pub(crate) struct LadderResult {
    pub(crate) writer_conn: Connection,
    /// What the reader thread opens: a plain file path for a file-backed
    /// store, or the same `cache=shared` memory URI the writer just created
    /// for a degraded one.
    pub(crate) reader_target: String,
    pub(crate) warning: Option<String>,
}

pub(crate) fn open_ladder(path: &Path) -> Result<LadderResult, Error> {
    // `reader_target` must round-trip through UTF-8 (A4/[rune-db 6] — the
    // same checked conversion every persisted path goes through, not
    // `to_string_lossy`): a mangled reader target would open the reader
    // thread against a DIFFERENT path than the writer's, silently. A
    // conversion failure here degrades to the next rung exactly like an
    // open failure would — `open_ladder` never hard-fails except at the
    // final in-memory rung.
    if let Ok(conn) = conn::open_recovery_store(RecoveryTarget::File(path))
        && let Ok(reader_target) = crate::paths::to_db_string(path)
    {
        return Ok(LadderResult {
            writer_conn: conn,
            reader_target,
            warning: None,
        });
    }

    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
        && let Ok(conn) = conn::open_recovery_store(RecoveryTarget::File(path))
        && let Ok(reader_target) = crate::paths::to_db_string(path)
    {
        return Ok(LadderResult {
            writer_conn: conn,
            reader_target,
            warning: None,
        });
    }

    let uri = conn::memory_uri();
    let writer_conn = conn::open_recovery_store(RecoveryTarget::Memory(&uri))?;
    Ok(LadderResult {
        writer_conn,
        reader_target: uri,
        warning: Some(DEGRADED_WARNING.to_string()),
    })
}
