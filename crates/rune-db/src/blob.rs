use rusqlite::{Connection, OptionalExtension, params};

use crate::Error;

pub(crate) fn put_blob(conn: &Connection, content: &[u8]) -> Result<String, Error> {
    let hash = hex_sha256(content);
    let compressed = zstd::encode_all(content, 0).map_err(Error::Io)?;

    conn.execute(
        "INSERT OR IGNORE INTO blobs(hash, content) VALUES(?1, ?2)",
        params![hash, compressed],
    )?;
    Ok(hash)
}

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

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    rune_vfs::etag_of(bytes).to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_support::open;

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
