use std::io;
use std::path::Path;

use crate::publish::{PutCondition, PutOutcome, put};
use crate::{Vfs, wrap_io_published};

/// Test-seeding convenience: NOT part of the materialize-complete primitive
/// set on `Vfs` (see `lib.rs`'s module docs) — every real caller composes
/// `write_durable`/`exchange`/`rename_excl` directly so it can capture
/// displaced bytes before they're discarded, which this cannot do (it
/// deletes the temp, and on the SWAP path the bytes the swap just displaced,
/// as its last step). Kept only so tests across the workspace can seed a
/// `Vfs` in one call.
pub trait VfsTestExt: Vfs {
    fn save_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let outcome = put(self, path, bytes, PutCondition::Force { expect: None })?;
        let durable = match outcome {
            PutOutcome::Committed { durable, .. } | PutOutcome::Raced { durable, .. } => durable,
            PutOutcome::Conflict { .. } | PutOutcome::Missing => {
                unreachable!("a Force put never conflicts or reports the destination missing")
            }
        };
        if durable {
            return Ok(());
        }
        Err(wrap_io_published(
            io::Error::other("durability could not be confirmed after publish"),
            "save published but durability could not be confirmed; \
             the prior content is preserved on a sibling temp file",
        ))
    }
}

impl<T: Vfs + ?Sized> VfsTestExt for T {}
