//! The initial-buffer load path split out of `main` (plan Context, WP5.S2):
//! reads the first positional's bytes through the injected `Vfs` and turns
//! them into a [`Buffer`], distinguishing "doesn't exist yet" from an
//! actual read failure and refusing invalid UTF-8 outright.

use std::path::Path;

use rune_core::buffer::{Buffer, BufferError};
use rune_vfs::Vfs;

#[derive(Debug)]
pub(crate) enum LoadError {
    InvalidUtf8,
    Io(std::io::Error),
}

/// A nonexistent path opens an empty buffer — it's created on first save via
/// `RENAME_EXCL` (plan Assumptions, A3). Any other read failure (permission
/// denied, a directory, ...) is fatal. Invalid UTF-8 is refused here, before
/// the TUI is ever entered. Reads through `vfs` (CONSTITUTION §1.4.9:
/// "Reach the filesystem only through the injected `vfs.FS`") rather than
/// `std::fs` directly, so this whole load path is exercisable against `Mem`
/// in tests, not just against a real disk.
pub(crate) fn load_buffer(vfs: &dyn Vfs, path: &Path) -> Result<Buffer, LoadError> {
    let bytes = match vfs.read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(LoadError::Io(e)),
    };
    Buffer::from_bytes(bytes).map_err(|e| match e {
        BufferError::InvalidUtf8 => LoadError::InvalidUtf8,
        // `from_bytes` only ever returns `InvalidUtf8` (see rune-core) — the
        // other `BufferError` variants come from `apply_edits`, never from
        // loading raw bytes. Still handled explicitly rather than assumed,
        // per CONSTITUTION §1.3 ("surface invalid input — no silent
        // fallback").
        other => LoadError::Io(std::io::Error::other(other.to_string())),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_vfs::Mem;

    #[test]
    fn load_buffer_reads_existing_file_through_the_vfs() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        vfs.save_atomic(path, b"hello").expect("seed the mem vfs");

        let buffer = load_buffer(&vfs, path).expect("existing file should load");
        assert_eq!(buffer.content(), "hello");
    }

    #[test]
    fn load_buffer_opens_empty_for_a_nonexistent_path() {
        let vfs = Mem::new();
        let buffer = load_buffer(&vfs, Path::new("/missing.md")).expect("missing path opens empty");
        assert!(buffer.is_empty());
    }

    #[test]
    fn load_buffer_refuses_invalid_utf8() {
        let vfs = Mem::new();
        let path = Path::new("/bad.md");
        vfs.save_atomic(path, &[0xff, 0xfe])
            .expect("seed the mem vfs");

        let err = load_buffer(&vfs, path).expect_err("invalid utf-8 must error");
        assert!(matches!(err, LoadError::InvalidUtf8));
    }
}
