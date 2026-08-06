use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_vfs::{Mem, Vfs};

pub fn new_app(content: &str) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let mut app = App::new(Buffer::new(content), None, vfs, None);
    app.frame_width = 80;
    app.frame_height = 24;
    app.relayout();
    app.sync_view();
    app
}
