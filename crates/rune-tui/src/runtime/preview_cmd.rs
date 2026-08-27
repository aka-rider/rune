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
        let result = (|| -> Result<Vec<u8>, CmdError> {
            let bytes = match rune_vfs::get(vfs.as_ref(), &path, MAX_PREVIEW_BYTES) {
                Ok(sighting) => sighting.bytes,
                Err(rune_vfs::GetRefusal::TooLarge { .. }) => {
                    return Err(CmdError::Refused("too large to preview".to_string()));
                }
                Err(e) => return Err(CmdError::from(e)),
            };
            if std::str::from_utf8(&bytes).is_err() {
                return Err(CmdError::Refused("not valid UTF-8".to_string()));
            }
            Ok(bytes)
        })();
        Some(Msg::FileOpened {
            path,
            result,
            anchor: None,
            preview_generation: Some(generation),
        })
    })
}
