//! The fuzzy file finder's own workspace walk `Cmd` — split out of
//! `runtime/mod.rs` (500-line budget), the same shape `highlight_cmd`/
//! `md_fence`/`snapshot_timer` already use for a distinct concern with its
//! own reason to exist as a file. `filesearch::open` is the sole caller.

use std::path::PathBuf;
use std::sync::Arc;

use rune_vfs::{FileKind, Vfs};

use super::{Cmd, Msg};
use crate::filesearch::walk;

/// Runs the ignore-aware recursive walk off-thread from `root`, replying
/// with `Msg::FileSearchScanned`. `generation` echoes `FileSearchState::
/// generation` — `filesearch::handle_scanned` drops a reply whose
/// generation no longer matches, the same shape `load_dir_cmd`'s own
/// `Msg::DirLoaded` uses. `root`'s own existence is checked here, before
/// handing off to `walk::scan` (which has no way to report "the root
/// itself doesn't exist" through its own `ScanResult`): a root that
/// vanished or resolved to a non-directory rides the same `Err` channel a
/// mid-walk unreadable subdirectory does NOT — `walk::scan` silently skips
/// those, since one bad subtree must never blank the whole finder, but the
/// root failing outright is worth telling the user about.
pub(crate) fn filesearch_scan_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    root: PathBuf,
    generation: u64,
) -> Cmd {
    Cmd::read_dir(move || {
        let result = match vfs.stat(&root) {
            Ok(stat) if stat.kind == FileKind::Dir => Ok(walk::scan(vfs.as_ref(), &root)),
            Ok(_) => Err(format!("{} is not a directory", root.display())),
            Err(e) => Err(format!("workspace root {} unreadable: {e}", root.display())),
        };
        Some(Msg::FileSearchScanned { generation, result })
    })
}
