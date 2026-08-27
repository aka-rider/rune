use std::path::PathBuf;
use std::sync::Arc;

use rune_vfs::{FileKind, Vfs};

use super::{Cmd, Msg};
use crate::filesearch::walk;

pub(crate) fn filesearch_scan_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    root: PathBuf,
    generation: crate::generation::FileSearchGen,
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
