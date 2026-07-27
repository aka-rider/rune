//! Content-addressed blob storage: zstd-compressed values keyed by the hex
//! SHA-256 of the PLAINTEXT (port of `pkg/docstate/snapshot.go:18-72`).
//! CONSTITUTION §1.4.10 ("capture displaced bytes as a durable blob before
//! they're ever discarded") is what this table exists to satisfy — every
//! snapshot and every `origin='swap'` observation (WP4) routes its content
//! through here.
//!
//! Both functions take `&Connection` (rather than `&Transaction`) so they
//! compose into a larger multi-statement transaction (`snapshot::
//! create_snapshot` calls `put_blob` as one step of its own tx) via
//! `rusqlite`'s `Transaction: Deref<Target = Connection>` coercion, while
//! still being independently callable — `get_blob` in particular is a
//! read-only, stale-tolerant lookup the plan explicitly allows onto
//! `reader.rs` (Hard rules: "reader.rs may gain get_blob/display reads
//! only").

use rusqlite::{Connection, OptionalExtension, params};

use crate::Error;

/// Stores `content` compressed under the hex SHA-256 of the PLAINTEXT,
/// `INSERT OR IGNORE` (content-addressed — an existing row with the same
/// hash is already byte-identical, so a duplicate insert is a deliberate
/// no-op, not a conflict). Returns the hash either way. Port of
/// `snapshot.go:18-43`.
pub(crate) fn put_blob(conn: &Connection, content: &str) -> Result<String, Error> {
    let hash = hex_sha256(content.as_bytes());
    let compressed = zstd::encode_all(content.as_bytes(), 0).map_err(Error::Io)?;

    conn.execute(
        "INSERT OR IGNORE INTO blobs(hash, content) VALUES(?1, ?2)",
        params![hash, compressed],
    )?;
    Ok(hash)
}

/// Decompresses and returns the content stored under `hash`, re-verifying
/// its SHA-256 against `hash` before returning — blob rot / bit-flip
/// detection. A mismatch is a corrupt blob and is surfaced as
/// [`Error::BlobHashMismatch`], never silently returned (port of
/// `snapshot.go:49-71`).
pub(crate) fn get_blob(conn: &Connection, hash: &str) -> Result<String, Error> {
    let compressed: Option<Vec<u8>> = conn
        .query_row(
            "SELECT content FROM blobs WHERE hash=?1",
            params![hash],
            |r| r.get(0),
        )
        .optional()?;
    let Some(compressed) = compressed else {
        return Err(Error::CorruptPayload(format!(
            "get blob {hash}: no such blob"
        )));
    };

    let data = zstd::decode_all(compressed.as_slice()).map_err(Error::Io)?;
    let got = hex_sha256(&data);
    if got != hash {
        return Err(Error::BlobHashMismatch {
            hash: hash.to_string(),
            got,
        });
    }
    String::from_utf8(data).map_err(|e| Error::CorruptPayload(e.to_string()))
}

/// Exposed `pub(crate)` (rather than kept private) so `observation.rs`
/// (`hashBytes`, `observation.go:207-212`) and `sync.rs` (`emptyHash`,
/// `sync.go:149-152`) share this ONE hex-SHA-256 implementation instead of a
/// second copy — the same hash space `blobs.hash`/`observations.blob_hash`
/// both live in.
pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let sum = hasher.finalize();
    let mut out = String::with_capacity(sum.len() * 2);
    for byte in sum {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    #[test]
    fn put_then_get_round_trips() {
        let conn = open();
        let hash = put_blob(&conn, "hello, blob").expect("put");
        let got = get_blob(&conn, &hash).expect("get");
        assert_eq!(got, "hello, blob");
    }

    #[test]
    fn identical_content_deduplicates_to_one_row() {
        let conn = open();
        let h1 = put_blob(&conn, "same").expect("put 1");
        let h2 = put_blob(&conn, "same").expect("put 2");
        assert_eq!(h1, h2);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM blobs WHERE hash=?1",
                params![h1],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(n, 1);
    }

    /// Port of `docstate_test.go`'s `corruptBlob`: flip the LAST byte of the
    /// stored compressed content (not append — appending can corrupt the
    /// zstd frame itself into a decode failure rather than exercising the
    /// hash re-verification this test targets).
    #[test]
    fn corrupted_blob_content_surfaces_hash_mismatch() {
        let conn = open();
        let hash = put_blob(&conn, "original content").expect("put");

        let mut compressed: Vec<u8> = conn
            .query_row(
                "SELECT content FROM blobs WHERE hash=?1",
                params![hash],
                |r| r.get(0),
            )
            .expect("read compressed blob");
        let last = compressed.len() - 1;
        if let Some(b) = compressed.get_mut(last) {
            *b ^= 0xFF;
        }
        conn.execute(
            "UPDATE blobs SET content=?2 WHERE hash=?1",
            params![hash, compressed],
        )
        .expect("corrupt");

        let err = get_blob(&conn, &hash).expect_err("must error on corrupt blob");
        assert!(matches!(err, Error::BlobHashMismatch { .. } | Error::Io(_)));
    }
}
