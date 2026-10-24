use std::path::PathBuf;
use std::sync::Arc;

use rune_vfs::Vfs;

use super::{Cmd, CmdError, Msg};

pub const MAX_PREVIEW_BYTES: u64 = 2 * 1024 * 1024;

pub fn read_preview_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    generation: crate::generation::PreviewGen,
) -> Cmd {
    Cmd::read_file(move || {
        let result = match rune_vfs::get(vfs.as_ref(), &path, MAX_PREVIEW_BYTES) {
            Ok(sighting) => Ok(sighting.bytes),
            Err(rune_vfs::GetRefusal::TooLarge { .. }) => {
                Err(CmdError::Refused("too large to preview".to_string()))
            }
            Err(e) => Err(CmdError::from(e)),
        };
        Some(Msg::FileOpened {
            path,
            result,
            anchor: None,
            preview_generation: Some(generation),
        })
    })
}
