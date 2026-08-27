//! Every error the crate's `wrap_io`/`wrap_io_published` chokepoint produces
//! must keep the original `io::Error` reachable via `std::error::Error::
//! source` — so a caller can still classify the ORIGINAL failure
//! (`kind()`, `raw_os_error()`) instead of that classification being erased
//! into a display string.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::error::Error;
use std::path::Path;

use rune_vfs::{Disk, Vfs};

#[test]
fn a_wrapped_disk_error_keeps_the_original_io_error_as_its_source() {
    let missing = Path::new("/nonexistent-rune-vfs-probe/gone.md");

    let err = Disk
        .read_link(missing)
        .expect_err("a missing path must error");

    let source = err
        .source()
        .expect("the wrapped error must keep the original error reachable as its source");
    let inner = source
        .downcast_ref::<std::io::Error>()
        .expect("the source must be the original io::Error");
    assert_eq!(inner.kind(), std::io::ErrorKind::NotFound);
}
