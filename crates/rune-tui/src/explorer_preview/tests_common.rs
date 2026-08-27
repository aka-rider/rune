#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer as CoreBuffer;
use rune_vfs::{DirEntry, FileKind, Mem, Vfs};

use super::*;
use crate::runtime::{DirCause, Msg};

pub(super) fn app_with(mem: &Arc<Mem>) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(CoreBuffer::new("hello"), None, vfs, None);
    app.active_doc_mut().viewport.set_size(80, 23);
    app.splits.left.show();
    app.frame = Some(crate::app::FrameSize::new(80, 24));
    app
}

pub(super) fn load_entries(app: &mut App, names: &[&str]) {
    let entries: Vec<DirEntry> = names
        .iter()
        .map(|name| DirEntry {
            name: (*name).to_string(),
            path: PathBuf::from("/root").join(name),
            kind: FileKind::File,
            link: rune_vfs::Link::No,
        })
        .collect();
    crate::explorer::handle_dir_loaded(
        app,
        PathBuf::from("/root"),
        entries,
        DirCause::Nav,
        crate::generation::Generation::ZERO,
    );
}

pub(super) fn run_cmds(app: &mut App, effects: &mut Effects) {
    let cmds = std::mem::take(&mut effects.cmds);
    for cmd in cmds {
        if let Some(Msg::FileOpened {
            path,
            result,
            anchor,
            preview_generation,
        }) = cmd.run()
        {
            workspace::handle_file_opened(app, &path, result, anchor, preview_generation, effects);
        }
    }
}

pub(super) fn run_cmds_through_update(app: &mut App, effects: &mut Effects) {
    let cmds = std::mem::take(&mut effects.cmds);
    for cmd in cmds {
        if let Some(msg) = cmd.run() {
            crate::app::update(app, msg, effects);
        }
    }
}
