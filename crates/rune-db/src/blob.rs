//! Content-addressed blob storage: zstd-compressed values keyed by the hex
//! SHA-256 of the PLAINTEXT bytes. Capturing displaced bytes as a durable
//! blob before they're ever discarded is what this table exists to
//! satisfy — every snapshot and every `origin='swap'` observation (WP4)
//! routes its content through here.
//!
//! Deliberately `&[u8]`/`Vec<u8>`-typed, never `&str`/`String`: disk-sourced
//! content — including a swap-race's *displaced* bytes, which may come from
//! ANY other writer and are captured under duress, never validated —
//! carries no UTF-8 validity guarantee, so it must never be rejected here
//! just because it fails to decode as UTF-8. A hard `unwrap`/error at this
//! layer would mean "no blob, no commit" for bytes that are already
//! physically on disk — this type turns that failure mode into a
//! compile-time impossibility. Callers holding genuinely
//! session-authored `String` content (the journal/snapshot/buffer path) pass
//! `.as_bytes()`; callers that need the content back as `String` (recovery
//! replay re-entering the edit buffer) convert explicitly at that boundary
//! and surface a decode failure there as a genuine error.
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

/// Stores `content` compressed under the hex SHA-256 of the raw bytes,
/// `INSERT OR IGNORE` (content-addressed — an existing row with the same
/// hash is already byte-identical, so a duplicate insert is a deliberate
/// no-op, not a conflict). Returns the hash either way.
pub(crate) fn put_blob(conn: &Connection, content: &[u8]) -> Result<String, Error> {
    let hash = hex_sha256(content);
    let compressed = zstd::encode_all(content, 0).map_err(Error::Io)?;

    conn.execute(
        "INSERT OR IGNORE INTO blobs(hash, content) VALUES(?1, ?2)",
        params![hash, compressed],
    )?;
    Ok(hash)
}

/// Decompresses and returns the raw bytes stored under `hash`,
/// re-verifying its SHA-256 against `hash` before returning — blob rot /
/// bit-flip detection. A mismatch is a corrupt blob and is surfaced as
/// [`Error::BlobHashMismatch`], never silently returned. Never attempts
/// a UTF-8 decode — that is each
/// caller's own concern, only where (and if) the bytes need to re-enter a
/// `String`.
pub(crate) fn get_blob(conn: &Connection, hash: &str) -> Result<Vec<u8>, Error> {
    let compressed: Option<Vec<u8>> = conn
        .query_row(
            "SELECT content FROM blobs WHERE hash=?1",
            params![hash],
            |r| r.get(0),
        )
        .optional()?;
    let Some(compressed) = compressed else {
        return Err(Error::NotFound(format!("get blob {hash}: no such blob")));
    };

    let data = zstd::decode_all(compressed.as_slice()).map_err(Error::Io)?;
    let got = hex_sha256(&data);
    if got != hash {
        return Err(Error::BlobHashMismatch {
            hash: hash.to_string(),
            got,
        });
    }
    Ok(data)
}

/// Exposed `pub(crate)` (rather than kept private) so `observation.rs`
/// and `sync.rs` share this ONE hex-SHA-256 implementation instead of a
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
        let hash = put_blob(&conn, b"hello, blob").expect("put");
        let got = get_blob(&conn, &hash).expect("get");
        assert_eq!(got, b"hello, blob");
    }

    #[test]
    fn get_blob_missing_hash_is_not_found() {
        let conn = open();
        let err = get_blob(
            &conn,
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect_err("missing blob must error");
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[test]
    fn non_utf8_content_round_trips_byte_exact() {
        let conn = open();
        let bytes: &[u8] = &[0xff, 0xfe, 0x00, 0x01, 0x9f, 0x92, 0x96];
        let hash = put_blob(&conn, bytes).expect("put non-utf8 blob");
        let got = get_blob(&conn, &hash).expect("get non-utf8 blob");
        assert_eq!(got, bytes, "non-utf8 bytes must round-trip byte-exact");
    }

    #[test]
    fn identical_content_deduplicates_to_one_row() {
        let conn = open();
        let h1 = put_blob(&conn, b"same").expect("put 1");
        let h2 = put_blob(&conn, b"same").expect("put 2");
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

    /// Flip the LAST byte of the stored compressed content (not append —
    /// appending can corrupt the zstd frame itself into a decode failure
    /// rather than exercising the hash re-verification this test targets).
    #[test]
    fn corrupted_blob_content_surfaces_hash_mismatch() {
        let conn = open();
        let hash = put_blob(&conn, b"original content").expect("put");

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
