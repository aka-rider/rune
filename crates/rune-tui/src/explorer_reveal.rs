use std::path::Path;

use crate::app::App;
use crate::explorer::{ensure_visible, request_dir};
use crate::runtime::Effects;
use crate::workspace;

pub fn reveal(app: &mut App, path: &Path, effects: &mut Effects) {
    let Some(resolved) = workspace::resolve_or_report(app, path, "reveal")
        .map(crate::resolved::ResolvedPath::into_path_buf)
    else {
        return;
    };
    let Some(parent) = resolved.parent().map(Path::to_path_buf) else {
        return;
    };

    if !app.explorer.entries.is_empty() && parent == app.explorer.root {
        let found = app.explorer.entries.iter().position(|e| e.path == resolved);
        app.explorer.nav.cursor = found.unwrap_or(0);
        ensure_visible(app);
        return;
    }

    app.explorer.pending_reveal = Some(resolved);
    request_dir(app, parent, effects);
}
