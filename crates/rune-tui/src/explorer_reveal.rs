//! Points the Explorer at a given file, re-rooting the listing to that
//! file's own parent directory when the Explorer isn't already rooted
//! there, and lands the cursor exactly on it.
//!
//! Matches strictly on `DirEntry::path`, never `name` (`rune-vfs`'s own
//! doc on the two fields): `name` is lossy-decoded for display, so a
//! non-UTF-8 filename could never round-trip through a `name` comparison
//! back to the exact file it started from.

use std::path::Path;

use crate::app::App;
use crate::explorer::{ensure_visible, request_dir};
use crate::runtime::Effects;
use crate::workspace;

/// Points the Explorer at `path` and lands the cursor on it. When `path`'s
/// parent is already the Explorer's current root, the cursor moves
/// synchronously — no reload. Otherwise the Explorer re-roots to that
/// parent and the cursor lands on `path` once the listing arrives
/// (`explorer::handle_dir_loaded`, via `Explorer::pending_reveal`).
///
/// `path` is resolved through `workspace::resolve` — the same chokepoint
/// `open_path` normalizes through — exactly once, and that single resolved
/// value is reused for the root comparison, the entry match, and
/// `pending_reveal`. Comparing an unresolved caller path against entries
/// straight from a `Vfs` listing (always resolved) would silently miss —
/// a symlink, a `..` segment, or a `./`-prefixed path would look identical
/// to the file having been deleted.
///
/// A `path` with no parent (nothing to root the listing at) is a no-op.
/// A `path` absent from the resulting listing (deleted, filtered, whatever)
/// falls back to the top of the list — the same rule `handle_dir_loaded`
/// already applies to a `Refresh` whose preserved selection vanished.
pub fn reveal(app: &mut App, path: &Path, effects: &mut Effects) {
    let resolved = workspace::resolve(app.vfs.as_ref(), path);
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
