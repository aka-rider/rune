//! The initial-buffer load path split out of `main` (plan Context, WP5.S2):
//! takes the first positional's single disk sighting through the injected
//! `Vfs`, distinguishing "doesn't exist yet" from an actual read failure
//! and refusing invalid UTF-8 outright.

use std::path::Path;

use rune_vfs::{GetRefusal, MAX_DOCUMENT_BYTES, Sighting, Vfs};

#[derive(Debug)]
pub(crate) enum LoadError {
    InvalidUtf8,
    Io(std::io::Error),
}

/// A disk sighting whose bytes have already been validated as UTF-8 — the
/// decoded `text` and the original [`Sighting`] (stat, etag, confirmed)
/// travel together so a caller building the initial buffer from `text` and
/// handing `sighting` on to the recovery store as its CAS baseline never
/// needs to re-decode or re-validate either one.
#[derive(Debug)]
pub(crate) struct LoadedFile {
    pub sighting: Sighting,
    pub text: String,
}

/// The single disk sighting a launch's first positional gets: `None` means
/// the path doesn't exist yet — created on first save via `RENAME_EXCL` —
/// `Some` carries the WHOLE [`LoadedFile`] rather than discarding everything
/// but the bytes, so the caller can hand the same sighting on to the
/// recovery store as its CAS baseline instead of taking a second, distinct
/// read of the same path. Any read failure other than not-found is fatal.
/// Invalid UTF-8 is refused here, before the TUI is ever entered, and
/// `text` is exactly this validation's own decode — a caller can never need
/// to re-validate it. Reads through `vfs` rather than `std::fs` directly, so
/// this whole load path is exercisable against `Mem` in tests, not just
/// against a real disk.
pub(crate) fn load_sighting(vfs: &dyn Vfs, path: &Path) -> Result<Option<LoadedFile>, LoadError> {
    match rune_vfs::get(vfs, path, Some(MAX_DOCUMENT_BYTES)) {
        Ok(sighting) => match String::from_utf8(sighting.bytes.clone()) {
            Ok(text) => Ok(Some(LoadedFile { sighting, text })),
            Err(_) => Err(LoadError::InvalidUtf8),
        },
        Err(GetRefusal::NotFound) => Ok(None),
        Err(refusal) => Err(LoadError::Io(refusal.into())),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_vfs::Mem;

    #[test]
    fn load_sighting_reads_existing_file_through_the_vfs() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        vfs.save_atomic(path, b"hello").expect("seed the mem vfs");

        let loaded = load_sighting(&vfs, path)
            .expect("existing file should load")
            .expect("must be Some for an existing file");
        assert_eq!(loaded.text, "hello");
    }

    #[test]
    fn load_sighting_is_none_for_a_nonexistent_path() {
        let vfs = Mem::new();
        let sighting =
            load_sighting(&vfs, Path::new("/missing.md")).expect("missing path is not an error");
        assert!(sighting.is_none());
    }

    #[test]
    fn load_sighting_refuses_invalid_utf8() {
        let vfs = Mem::new();
        let path = Path::new("/bad.md");
        vfs.save_atomic(path, &[0xff, 0xfe])
            .expect("seed the mem vfs");

        let err = load_sighting(&vfs, path).expect_err("invalid utf-8 must error");
        assert!(matches!(err, LoadError::InvalidUtf8));
    }
}
