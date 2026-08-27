#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::ids::Seq;
use rusqlite::ffi;

fn sqlite_error(message: &str) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(ffi::Error::new(1), Some(message.to_string()))
}

fn json_error(bad_json: &str) -> serde_json::Error {
    serde_json::from_str::<serde_json::Value>(bad_json)
        .expect_err("malformed json must fail to parse")
}

fn utf8_error() -> std::string::FromUtf8Error {
    String::from_utf8(vec![0xff]).expect_err("0xff alone is not valid utf-8")
}

#[test]
fn session_establish_reason_no_start_time_pins_message() {
    let reason = SessionEstablishReason::NoStartTime { pid: 42 };
    assert_eq!(
        reason.to_string(),
        "could not read start time of own pid 42"
    );
}

/// `SessionEstablishReason::Sqlite` just forwards to the inner error's own
/// `Display` — pin that the forwarding is exact by comparing against a
/// second, identically-constructed `rusqlite::Error` rather than a literal,
/// since the wording of `rusqlite::Error`'s own `Display` is not ours to pin.
#[test]
fn session_establish_reason_sqlite_pins_inner_message() {
    let expected = sqlite_error("insert failed").to_string();
    let reason = SessionEstablishReason::Sqlite(sqlite_error("insert failed"));
    assert_eq!(reason.to_string(), expected);
}

#[test]
fn corrupt_payload_reason_non_utf8_blob_pins_message() {
    let reason = CorruptPayloadReason::NonUtf8Blob {
        hash: "h1".to_string(),
        doc_id: DocId(7),
        source: utf8_error(),
    };
    assert_eq!(
        reason.to_string(),
        format!(
            "snapshot blob h1 for doc 7: non-utf8 content: {}",
            utf8_error()
        )
    );
}

#[test]
fn corrupt_payload_reason_json_pins_inner_message() {
    let expected = json_error("not json").to_string();
    let reason = CorruptPayloadReason::Json(json_error("not json"));
    assert_eq!(reason.to_string(), expected);
}

#[test]
fn corrupt_payload_reason_invalid_cursor_id_pins_message() {
    let reason = CorruptPayloadReason::InvalidCursorId { id: 0 };
    assert_eq!(reason.to_string(), "cursor id 0 must be non-zero");
}

#[test]
fn replay_failure_pins_message() {
    let failure = ReplayFailure {
        doc_id: DocId(3),
        session_id: SessionId(9),
        seq: Seq(5),
        source: rune_core::buffer::BufferError::InvalidUtf8,
    };
    assert_eq!(
        failure.to_string(),
        "doc 3 session 9 at seq 5: invalid UTF-8 sequence"
    );
}

#[test]
fn error_display_pins_sqlite_and_io_variants() {
    assert_eq!(
        Error::Sqlite(sqlite_error("boom")).to_string(),
        format!("sqlite: {}", sqlite_error("boom"))
    );
    let io = std::io::Error::other("disk gone");
    assert_eq!(Error::Io(io).to_string(), "io: disk gone");
}

#[test]
fn error_display_pins_the_unit_and_message_variants() {
    assert_eq!(Error::WriterQueueFull.to_string(), "writer queue full");
    assert_eq!(Error::WriterGone.to_string(), "writer thread is gone");
    assert_eq!(Error::ReaderGone.to_string(), "reader thread is gone");
    assert_eq!(
        Error::NotFound("doc 9".to_string()).to_string(),
        "not found: doc 9"
    );
    assert_eq!(
        Error::Invalid("bad path".to_string()).to_string(),
        "bad path"
    );
}

#[test]
fn error_display_pins_wal_mode_unavailable() {
    assert_eq!(
        Error::WalModeUnavailable("delete".to_string()).to_string(),
        "PRAGMA journal_mode=WAL returned \"delete\", not \"wal\""
    );
}

#[test]
fn error_display_pins_blob_hash_mismatch() {
    assert_eq!(
        Error::BlobHashMismatch {
            hash: "abc".to_string(),
            got: "def".to_string(),
        }
        .to_string(),
        "get blob abc: content hash mismatch (corrupt blob): got def"
    );
}

#[test]
fn error_display_pins_session_establish_and_corrupt_payload_wrapping() {
    let reason = SessionEstablishReason::NoStartTime { pid: 1 };
    assert_eq!(
        Error::SessionEstablish(reason).to_string(),
        "establish session: could not read start time of own pid 1"
    );
    let reason = CorruptPayloadReason::InvalidCursorId { id: 0 };
    assert_eq!(
        Error::CorruptPayload(reason).to_string(),
        "corrupt journal payload: cursor id 0 must be non-zero"
    );
}

#[test]
fn error_display_pins_replay_failed_wrapping() {
    let failure = ReplayFailure {
        doc_id: DocId(1),
        session_id: SessionId(2),
        seq: Seq(3),
        source: rune_core::buffer::BufferError::InvalidUtf8,
    };
    assert_eq!(
        Error::ReplayFailed(Box::new(failure)).to_string(),
        "replay failed: doc 1 session 2 at seq 3: invalid UTF-8 sequence"
    );
}

#[test]
fn error_source_sqlite_returns_the_inner_error() {
    let inner_message = sqlite_error("boom").to_string();
    let err = Error::Sqlite(sqlite_error("boom"));
    let source = std::error::Error::source(&err).expect("Sqlite must report a source");
    assert_eq!(source.to_string(), inner_message);
}

#[test]
fn error_source_io_returns_the_inner_error() {
    let err = Error::Io(std::io::Error::other("disk gone"));
    let source = std::error::Error::source(&err).expect("Io must report a source");
    assert_eq!(source.to_string(), "disk gone");
}

#[test]
fn error_source_session_establish_sqlite_returns_the_inner_error() {
    let inner_message = sqlite_error("insert failed").to_string();
    let err = Error::SessionEstablish(SessionEstablishReason::Sqlite(sqlite_error(
        "insert failed",
    )));
    let source =
        std::error::Error::source(&err).expect("SessionEstablish(Sqlite) must report a source");
    assert_eq!(source.to_string(), inner_message);
}

/// `SessionEstablish(NoStartTime)` has no wrapped error at all — pins that
/// this arm falls through to `_ => None` rather than accidentally matching
/// a mutated, broader `SessionEstablish(_)` pattern.
#[test]
fn error_source_session_establish_no_start_time_is_none() {
    let err = Error::SessionEstablish(SessionEstablishReason::NoStartTime { pid: 1 });
    assert!(std::error::Error::source(&err).is_none());
}

#[test]
fn error_source_corrupt_payload_non_utf8_blob_returns_the_inner_error() {
    let inner_message = utf8_error().to_string();
    let err = Error::CorruptPayload(CorruptPayloadReason::NonUtf8Blob {
        hash: "h".to_string(),
        doc_id: DocId(1),
        source: utf8_error(),
    });
    let source = std::error::Error::source(&err).expect("NonUtf8Blob must report a source");
    assert_eq!(source.to_string(), inner_message);
}

#[test]
fn error_source_corrupt_payload_json_returns_the_inner_error() {
    let inner_message = json_error("not json").to_string();
    let err = Error::CorruptPayload(CorruptPayloadReason::Json(json_error("not json")));
    let source = std::error::Error::source(&err).expect("Json must report a source");
    assert_eq!(source.to_string(), inner_message);
}

/// `CorruptPayload(InvalidCursorId)` has no wrapped error at all — pins the
/// fallthrough distinctly from the two `CorruptPayload` arms that do.
#[test]
fn error_source_corrupt_payload_invalid_cursor_id_is_none() {
    let err = Error::CorruptPayload(CorruptPayloadReason::InvalidCursorId { id: 0 });
    assert!(std::error::Error::source(&err).is_none());
}

#[test]
fn error_source_replay_failed_returns_the_inner_error() {
    let failure = ReplayFailure {
        doc_id: DocId(1),
        session_id: SessionId(2),
        seq: Seq(3),
        source: rune_core::buffer::BufferError::InvalidUtf8,
    };
    let inner_message = failure.source.to_string();
    let err = Error::ReplayFailed(Box::new(failure));
    let source = std::error::Error::source(&err).expect("ReplayFailed must report a source");
    assert_eq!(source.to_string(), inner_message);
}

/// Variants with no source-bearing arm at all pin that `source()`'s
/// fallthrough is reached for real, not merely never exercised — this is
/// what tells "deleted one specific match arm" apart from "source always
/// returns None" together with the `is_some` tests above.
#[test]
fn error_source_is_none_for_variants_without_a_wrapped_error() {
    assert!(std::error::Error::source(&Error::WriterQueueFull).is_none());
    assert!(std::error::Error::source(&Error::WriterGone).is_none());
    assert!(std::error::Error::source(&Error::ReaderGone).is_none());
    assert!(std::error::Error::source(&Error::NotFound("x".to_string())).is_none());
    assert!(std::error::Error::source(&Error::Invalid("x".to_string())).is_none());
}
