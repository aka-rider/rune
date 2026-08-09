//! Opening a path that is not a regular file (a FIFO/socket/device node,
//! `FileKind::Other`) must refuse with an error message instead of reading
//! it — a synchronous read of a FIFO inside `update` would block the whole
//! Elm loop.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::runtime::Effects;
use rune_tui::workspace;
use rune_vfs::{FileKind, Mem, Vfs};

fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

#[test]
fn opening_a_non_file_from_the_explorer_posts_an_error_and_opens_nothing() {
    let mem = Arc::new(Mem::new());
    publish(&mem, Path::new("/fifo.md"), b"");
    mem.set_kind(Path::new("/fifo.md"), FileKind::Other)
        .expect("set_kind");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as _;

    let mut app = App::new(Buffer::new(""), None, vfs, None);
    app.frame_width = 80;
    app.frame_height = 24;
    let docs_before = app.tabs.order.len();

    let mut effects = Effects::default();
    let opened = workspace::open_path_checked(&mut app, Path::new("/fifo.md"), &mut effects);

    assert!(opened.is_none(), "a non-file must never open a document");
    assert_eq!(
        app.tabs.order.len(),
        docs_before,
        "no new tab may appear for a refused open"
    );
    let log = rune_tui::messages::log_text(&app);
    assert!(
        log.contains("could not open") && log.contains("not a regular file"),
        "the refusal must be surfaced in the message log: {log:?}"
    );
}
