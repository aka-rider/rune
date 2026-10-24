//! The initial-buffer load path: takes the first positional's single disk
//! sighting through the injected `Vfs`, distinguishing "doesn't exist yet"
//! from an actual read failure.

use std::path::Path;

use rune_core::codec;
use rune_vfs::{GetRefusal, MAX_DOCUMENT_BYTES, Sighting, Vfs};

/// A disk sighting with its bytes already decoded — the decoded `text` and
/// the original [`Sighting`] (stat, etag, confirmed) travel together so a
/// caller building the initial buffer from `text` and handing `sighting` on
/// to the recovery store as its CAS baseline never decodes either one again.
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
/// Reads through `vfs` rather than `std::fs` directly, so
/// this whole load path is exercisable against `Mem` in tests, not just
/// against a real disk.
///
/// `path` MUST already be the caller's own `Vfs::resolve` output (`main`'s
/// `open_launch` resolves the launch positional exactly once, before this
/// runs) — this calls [`rune_vfs::get_resolved`], not [`rune_vfs::get`], so
/// it never resolves `path` a second time. A second, independent resolve is
/// a symlink-swap TOCTOU window: the identity the caller decided to open
/// could stop being the identity this function reads.
pub(crate) fn load_sighting(
    vfs: &dyn Vfs,
    path: &Path,
) -> Result<Option<LoadedFile>, std::io::Error> {
    match rune_vfs::get_resolved(vfs, path, MAX_DOCUMENT_BYTES) {
        Ok(sighting) => {
            let text = codec::decode(sighting.bytes.clone());
            Ok(Some(LoadedFile { sighting, text }))
        }
        Err(GetRefusal::NotFound) => Ok(None),
        Err(refusal) => Err(refusal.into()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_vfs::{Mem, VfsTestExt};

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
    fn load_sighting_decodes_invalid_utf8_to_raw_chars() {
        let vfs = Mem::new();
        let path = Path::new("/bad.md");
        vfs.save_atomic(path, &[0xff, 0xfe])
            .expect("seed the mem vfs");

        let loaded = load_sighting(&vfs, path)
            .expect("invalid utf-8 must load")
            .expect("must be Some for an existing file");
        let expected: String = [0xff, 0xfe]
            .into_iter()
            .map(|b| codec::raw_byte_char(b).expect("raw byte char"))
            .collect();
        assert_eq!(loaded.text, expected);
        assert_eq!(loaded.sighting.bytes, [0xff, 0xfe]);
    }
}
