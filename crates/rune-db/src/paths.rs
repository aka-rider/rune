//! The ONE checked `Path -> String` conversion for persistence (A4).
//!
//! Every path-shaped column this crate persists (`documents.path`,
//! `session_documents`'s indirect path references via `documents`, etc.) is
//! `TEXT` — a deliberate decision (A4) to avoid a schema migration to `BLOB`
//! for the sake of paths so pathological they aren't valid UTF-8. That
//! decision only holds if a non-round-tripping path is REJECTED loudly at
//! the point it would otherwise enter a `TEXT` column, rather than silently
//! mangled: `to_string_lossy()` substitutes U+FFFD for any invalid byte
//! sequence, which then persists, resolves back to a DIFFERENT path than the
//! one on disk, and the document becomes permanently unsavable with no
//! surfaced explanation ([rune-db 6]) — the exact failure mode this
//! chokepoint exists to make unreachable. Every one of this crate's five
//! former `to_string_lossy()` persistence sites (`document.rs`,
//! `materialize.rs`, `rename.rs` ×2, `store.rs` ×2) now routes through
//! [`to_db_string`] instead.

use std::path::Path;

use crate::Error;

/// Converts `path` to an owned `String` for a `TEXT` column, or a typed
/// error if `path` does not round-trip through UTF-8. Documents at such a
/// path get no recovery store (A4's accepted cost) — they still open/edit/
/// save via `rune-vfs` directly, which is byte-exact throughout.
pub(crate) fn to_db_string(path: &Path) -> Result<String, Error> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        Error::Invalid(format!(
            "{}: path is not valid UTF-8 — cannot be tracked in the recovery store",
            path.display()
        ))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn valid_utf8_path_round_trips() {
        let got = to_db_string(Path::new("/a/b/café.md")).expect("must convert");
        assert_eq!(got, "/a/b/café.md");
    }

    #[test]
    fn non_utf8_path_is_rejected_with_a_typed_error() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let bytes: &[u8] = &[0x2f, 0xff, 0xfe, 0x2e, 0x6d, 0x64]; // "/\xFF\xFE.md"
        let os = OsStr::from_bytes(bytes);
        let path = Path::new(os);

        let err = to_db_string(path).expect_err("non-utf8 path must be rejected");
        assert!(matches!(err, Error::Invalid(_)));
    }
}
