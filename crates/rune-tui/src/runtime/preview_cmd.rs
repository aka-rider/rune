use std::path::PathBuf;
use std::sync::Arc;

use rune_vfs::Vfs;

use super::{Cmd, CmdError, Msg};

/// The Explorer live-preview's largest previewable file, in bytes — past
/// this, `explorer_preview` skips the read entirely rather than pulling a
/// huge file into memory just because the cursor happened to pass over it.
pub const MAX_PREVIEW_BYTES: u64 = 2 * 1024 * 1024;

/// Reads `path` off-thread for the Explorer's live preview, the same
/// physical work as [`super::read_file_cmd`] but with the preview's own tighter
/// size gate ([`MAX_PREVIEW_BYTES`], enforced by `rune_vfs::get` before
/// reading) and a UTF-8 validity check so a binary file never reaches
/// `Buffer::from_bytes`. Every rejection reports through the SAME `Result`
/// channel `read_file_cmd` uses (`Msg::FileOpened`'s `result`) rather than a
/// distinct error shape — `explorer_preview::maybe_consume_reply` is the
/// only reader, and it treats every `Err` here identically: silently keep
/// showing whatever was previewed before, never the ordinary open-failure
/// banner `workspace::handle_file_opened` would otherwise raise. `anchor`
/// is always `None`: a preview never lands a navigation anchor.
pub fn read_preview_cmd(vfs: Arc<dyn Vfs + Send + Sync>, path: PathBuf) -> Cmd {
    Cmd::read_file(move || {
        let result = (|| -> Result<Vec<u8>, CmdError> {
            let bytes = match rune_vfs::get(vfs.as_ref(), &path, Some(MAX_PREVIEW_BYTES)) {
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
        })
    })
}
