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

/// The single disk sighting a launch's first positional gets (issue #77):
/// `None` means the path doesn't exist yet — created on first save via
/// `RENAME_EXCL` (plan Assumptions, A3) — `Some` carries the WHOLE
/// [`Sighting`] (bytes, stat, confirmed) rather than discarding everything
/// but the bytes, so the caller can hand the SAME sighting on to the
/// recovery store as its CAS baseline instead of taking a second, distinct
/// read of the same path. Any read failure other than not-found is fatal.
/// Invalid UTF-8 is refused here, before the TUI is ever entered. Reads
/// through `vfs` rather than `std::fs` directly, so this whole load path is
/// exercisable against `Mem` in tests, not just against a real disk.
pub(crate) fn load_sighting(vfs: &dyn Vfs, path: &Path) -> Result<Option<Sighting>, LoadError> {
    match rune_vfs::get(vfs, path, Some(MAX_DOCUMENT_BYTES)) {
        Ok(sighting) if std::str::from_utf8(&sighting.bytes).is_ok() => Ok(Some(sighting)),
        Ok(_) => Err(LoadError::InvalidUtf8),
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

        let sighting = load_sighting(&vfs, path)
            .expect("existing file should load")
            .expect("must be Some for an existing file");
        assert_eq!(sighting.bytes, b"hello");
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
