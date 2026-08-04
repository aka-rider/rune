//! The Explorer pane: a `Vfs::read_dir`-backed file/directory list (plan
//! WP4.S3), navigable via `listnav::List`. `Pane::Explorer`-focused key
//! handling lives in the sibling `explorer_keys` module (split out per
//! §1.6); row layout lives here (`draw`, delegated to from
//! `render.rs::draw_left_pane` — the bordered `Block`/focus-colored border
//! stays render.rs's job, plan WP4.S6).
//!
//! Directory loading is a boundary `Msg` (plan WP4.S4, decision 10:
//! same-tick pane actions are direct calls, but crossing a thread to read
//! the filesystem is not): `runtime::load_dir_cmd` runs `vfs.read_dir` off-
//! thread and replies with `Msg::DirLoaded`, handled here by
//! `handle_dir_loaded` (routed from `app::update_inner`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use rune_vfs::DirEntry;

use crate::app::App;
use crate::listnav;
use crate::runtime::{DirCause, Effects, load_dir_cmd};
use crate::width::truncate_tail_to_width;

/// One open Explorer's state (plan WP4.S3): the directory it's rooted at,
/// its direct children (dirs-first, `Vfs::read_dir`'s own sort contract),
/// the cursor/scroll position, and whether a `ReadDir` `Cmd` is in flight.
/// `root` starts empty — the real starting root (the active document's own
/// directory, or the process's cwd for a pathless session) is resolved
/// through `app.vfs` at the FIRST `^x` toggle (`initial_root`), never at
/// `App::new`, so constructing an `App` never touches the filesystem.
pub struct Explorer {
    pub root: PathBuf,
    pub entries: Vec<DirEntry>,
    pub nav: listnav::List,
    pub loading: bool,
    /// Bumped at every `ReadDir` `Cmd` this Explorer issues (`request_dir`
    /// below, and `pane::handle_global_command`'s initial `^x` load) — the
    /// generation token `Msg::DirLoaded` carries back, so an OLDER in-flight
    /// reply can never overwrite a newer listing. Mirrors `DocDb::snapshot_
    /// generation`'s debounce-token pattern (`db.rs`).
    pub request_generation: u32,
    pub search: Option<String>, // type-to-search query (`explorer_search`); None = inactive
    /// The file `explorer_reveal::reveal` wants the cursor on, set when it
    /// issues a re-rooting `ReadDir`, consumed by `handle_dir_loaded` below
    /// — guarded by `request_generation` like every other in-flight one.
    pub pending_reveal: Option<PathBuf>,
}

impl Default for Explorer {
    fn default() -> Explorer {
        Explorer {
            root: PathBuf::new(),
            entries: Vec::new(),
            nav: listnav::List { cursor: 0, top: 0 },
            loading: false,
            request_generation: 0,
            search: None,
            pending_reveal: None,
        }
    }
}

/// The Explorer's starting root the first time it's ever shown (plan
/// WP4.S4's "`^x` triggers the initial load"): the active document's own
/// directory takes priority (deliberate, and different from Go, which
/// always roots the tree at the workspace root) — a pathless (draft)
/// session falls back to `app.root` (the workspace root discovered at
/// startup), and only when THAT is also unresolved (still empty) does this
/// fall back to the literal `"."`. Resolved through `app.vfs` (§1.4.9) so
/// `Disk` canonicalizes it exactly like every other filesystem entry point;
/// `Mem` (tests) normalizes it lexically to its own synthetic root (`Mem`
/// has no real cwd to canonicalize `"."` against).
pub fn initial_root(app: &App) -> PathBuf {
    let base = app
        .active_doc()
        .file_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            if app.root.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                app.root.clone()
            }
        });
    app.vfs.resolve(&base).unwrap_or(base)
}

/// Scrolls the Explorer's window to keep the cursor visible (plan WP4.S3:
/// "follow, margin = min(4, visible/4) like Go filetree" — Go's own
/// `ensureVisible`, jump
/// buffer 0 since Go's own `Follow` call has no jump argument either).
/// `pub(crate)`, not private: `explorer_keys::handle_key`'s Top/Bottom
/// commands and `move_selection` call this from the sibling module the key
/// handling lives in.
pub(crate) fn ensure_visible(app: &mut App) {
    let len = app.explorer.entries.len();
    let height = visible_rows(app);
    let margin = (height / 4).min(4);
    app.explorer.nav.follow(len, height, margin, 0);
}

/// The Explorer pane's visible entry-row count for `ensure_visible`'s
/// scroll margin — read straight from `layout::geometry` (plan WP3.S7),
/// the one chokepoint every pane's rect comes from, rather than the
/// `viewport.height`-based approximation this used to reverse-engineer.
/// The `-1` is the root-path row (`draw` below, row 0 of the block's inner
/// rect) that isn't available for entries.
fn visible_rows(app: &App) -> usize {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    (crate::layout::geometry(area, app).explorer_inner.height as usize)
        .saturating_sub(1)
        .max(1)
}

/// Issues the `ReadDir` `Cmd` that (re)lists `root`. `pub(crate)`, not
/// private: `explorer_keys::open_selected`/`go_to_parent` call this from
/// the sibling module the key handling lives in, exactly like
/// `ensure_loaded`/`refresh_for` below do from this one.
pub(crate) fn request_dir(app: &mut App, root: PathBuf, effects: &mut Effects) {
    app.explorer.loading = true;
    app.explorer.request_generation = app.explorer.request_generation.wrapping_add(1);
    let generation = app.explorer.request_generation;
    let vfs = Arc::clone(&app.vfs);
    effects
        .cmds
        .push(load_dir_cmd(vfs, root, DirCause::Nav, generation));
}

/// Requests the Explorer's very first listing if it has none yet. The one
/// chokepoint for that first load, so every route that can put the pane in
/// front of the user — focusing it with a chord, or launching with no file
/// to edit, which shows the column before any key is pressed — fills it the
/// same way. Without this, a column shown at startup would render an empty
/// box with a blank root row until the user happened to press the focus
/// chord.
///
/// "Empty and not already loading" is the no-shadow-state stand-in for
/// "never loaded": `Explorer` carries no separate `loaded` flag, and a
/// genuinely empty directory re-triggering this is a harmless reload, not
/// an incorrect state. A hidden column is left alone — nothing is on screen
/// to fill.
pub fn ensure_loaded(app: &mut App, effects: &mut Effects) {
    if !app.splits.left.is_shown() || !app.explorer.entries.is_empty() || app.explorer.loading {
        return;
    }
    let root = initial_root(app);
    request_dir(app, root, effects);
}

/// Re-lists the Explorer when `path`'s parent IS its current root — the
/// post-rename side effect (a `pub(crate)` sibling of the private
/// `request_dir`). Uses `DirCause::Refresh` so the user's selected entry is
/// preserved by name rather than snapping back to the top: a rename is not
/// a navigation.
///
/// A no-op when the Explorer was never loaded (nothing on screen to go
/// stale) or when the renamed file lives somewhere else.
pub(crate) fn refresh_for(app: &mut App, path: &Path, effects: &mut Effects) {
    if app.explorer.entries.is_empty() {
        return; // never loaded; nothing to refresh
    }
    let Some(parent) = path.parent() else { return };
    if parent != app.explorer.root {
        return;
    }
    app.explorer.loading = true;
    app.explorer.request_generation = app.explorer.request_generation.wrapping_add(1);
    let generation = app.explorer.request_generation;
    let vfs = Arc::clone(&app.vfs);
    let root = app.explorer.root.clone();
    effects
        .cmds
        .push(load_dir_cmd(vfs, root, DirCause::Refresh, generation));
}

/// Prepends a synthetic `..` row to `entries` when `root` has a parent — a
/// REAL `DirEntry` carrying the real parent path, not a render-time overlay.
/// Because it's a genuine list element, `open_selected`'s existing
/// directory branch (resolve, then `request_dir`) already does exactly what
/// `go_to_parent` does when the user presses Enter on it — no `".."`
/// special case anywhere, and `listnav::List`'s cursor keeps addressing the
/// one real list it's always addressed, never an N+1 rendered one. A root
/// with no parent (a filesystem root) gets no such row.
fn with_parent_entry(root: &Path, mut entries: Vec<DirEntry>) -> Vec<DirEntry> {
    let Some(parent) = root.parent() else {
        return entries;
    };
    entries.insert(
        0,
        DirEntry {
            name: "..".to_string(),
            path: parent.to_path_buf(),
            is_dir: true,
        },
    );
    entries
}

/// Reacts to `Msg::DirLoaded` (plan WP4.S4), routed from `app::update_
/// inner`. A `generation` that no longer matches `app.explorer.request_
/// generation` is a reply to a SUPERSEDED request (a later `ReadDir` was
/// already issued — every issue site bumps the generation) and is ignored
/// outright. `Nav` resets the cursor to the top; `Refresh` keeps the
/// currently selected entry selected BY NAME when still present (falling
/// back to the top) — the shape `refresh_for` (above) uses on every
/// rename landing inside the current root. A pending reveal (see the field
/// doc on `Explorer::pending_reveal`) wins over both.
pub(crate) fn handle_dir_loaded(
    app: &mut App,
    root: PathBuf,
    entries: Vec<DirEntry>,
    cause: DirCause,
    generation: u32,
) {
    if generation != app.explorer.request_generation {
        return;
    }

    crate::explorer_search::clear_search(app); // a new listing outdates any query
    let entries = with_parent_entry(&root, entries);

    let reveal_target = app.explorer.pending_reveal.take();
    let preserve_name = match cause {
        DirCause::Nav => None,
        DirCause::Refresh => app
            .explorer
            .entries
            .get(app.explorer.nav.cursor)
            .map(|e| e.name.clone()),
    };

    app.explorer.root = root;
    app.explorer.entries = entries;
    app.explorer.loading = false;
    let by_reveal =
        reveal_target.and_then(|t| app.explorer.entries.iter().position(|e| e.path == t));
    let by_name = preserve_name.and_then(|n| app.explorer.entries.iter().position(|e| e.name == n));
    app.explorer.nav.cursor = by_reveal.or(by_name).unwrap_or(0);
    ensure_visible(app);
}

/// Draws the Explorer's content into `area` — the block's INNER rect
/// (border already rendered by `render.rs::draw_left_pane`, plan WP4.S6):
/// row 0 is the root path (truncated with a leading `…` when it doesn't
/// fit, `theme.chrome.pane_title`); the remaining rows are the `listnav`-
/// windowed entry slice, `>` prefixed and `theme.chrome.file_selected` on
/// the cursor row, `/` suffixed for a directory.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    if area.height == 0 {
        return;
    }
    let mut lines = Vec::with_capacity(area.height as usize);
    lines.push(Line::from(Span::styled(
        truncate_root(&app.explorer.root, area.width as usize),
        app.theme.chrome.pane_title,
    )));

    let entry_rows = (area.height as usize).saturating_sub(1);
    let window = app
        .explorer
        .nav
        .window(app.explorer.entries.len(), entry_rows);
    let start = window.start;
    let visible = app.explorer.entries.get(window).unwrap_or(&[]);
    for (i, entry) in visible.iter().enumerate() {
        let idx = start + i;
        let selected = idx == app.explorer.nav.cursor;
        let prefix = if selected { "\u{203a} " } else { "  " };
        let suffix = if entry.is_dir { "/" } else { "" };
        let style = if selected {
            app.theme.chrome.file_selected
        } else {
            app.theme.chrome.file_normal
        };
        lines.push(Line::from(Span::styled(
            format!("{prefix}{}{suffix}", entry.name),
            style,
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Truncates `root`'s displayed path to fit `width` terminal CELLS (§1.5),
/// keeping the TAIL and marking the cut with a leading `…` (plan WP4.S3:
/// "root-path title row truncated with leading `…`") — the tail (the
/// directory's own name and its nearest ancestors) is what a user
/// navigating a deep tree actually needs to see, not the common prefix
/// every row would otherwise share. Delegates to the crate's one chrome-
/// width chokepoint (`width::truncate_tail_to_width`) so a CJK root
/// component can't pass the fit check on a char count and then overrun the
/// cell budget it was supposed to respect.
fn truncate_root(root: &Path, width: usize) -> String {
    let text = root.display().to_string();
    truncate_tail_to_width(&text, width)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::app::App;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.active_doc_mut().viewport.set_size(80, 23);
        app
    }

    fn entries(names: &[(&str, bool)]) -> Vec<DirEntry> {
        names
            .iter()
            .map(|(name, is_dir)| DirEntry {
                name: (*name).to_string(),
                path: PathBuf::from(*name),
                is_dir: *is_dir,
            })
            .collect()
    }

    #[test]
    fn nav_load_resets_the_cursor_to_the_top() {
        let mut app = app();
        app.explorer.nav.cursor = 3;
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("b", false)]),
            DirCause::Nav,
            0,
        );
        assert_eq!(app.explorer.nav.cursor, 0);
        // "/root" has a parent ("/"), so a synthetic ".." row is prepended.
        assert_eq!(app.explorer.entries.len(), 3);
    }

    #[test]
    fn refresh_preserves_the_selected_entry_by_name() {
        let mut app = app();
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("b", false), ("c", false)]),
            DirCause::Nav,
            0,
        );
        app.explorer.nav.cursor = 3; // "c", shifted one place by the leading ".." row

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("new", false), ("a", false), ("c", false)]),
            DirCause::Refresh,
            0,
        );
        assert_eq!(app.explorer.entries[app.explorer.nav.cursor].name, "c");
    }

    #[test]
    fn refresh_falls_back_to_the_top_when_the_selection_vanished() {
        let mut app = app();
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("gone", false)]),
            DirCause::Nav,
            0,
        );
        app.explorer.nav.cursor = 2; // "gone", shifted one place by the leading ".." row

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("still-here", false)]),
            DirCause::Refresh,
            0,
        );
        assert_eq!(app.explorer.nav.cursor, 0);
    }

    /// A `DirLoaded` reply whose `generation` no longer matches the
    /// Explorer's current `request_generation` (a later request already
    /// superseded it) must be ignored outright — the review fix for two
    /// in-flight `ReadDir` Cmds landing out of order.
    #[test]
    fn a_stale_generation_reply_is_ignored() {
        let mut app = app();
        app.explorer.request_generation = 5;
        app.explorer.root = PathBuf::from("/root");
        app.explorer.entries = entries(&[("a", false)]);

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/elsewhere"),
            entries(&[("stale", false)]),
            DirCause::Nav,
            4, // superseded — the live generation is 5
        );

        assert_eq!(
            app.explorer.root,
            PathBuf::from("/root"),
            "a stale-generation reply must not overwrite the current listing"
        );
        assert_eq!(app.explorer.entries, entries(&[("a", false)]));
    }

    /// The reply carrying the CURRENT generation is applied normally.
    #[test]
    fn the_current_generation_reply_is_applied() {
        let mut app = app();
        app.explorer.request_generation = 5;

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/fresh"),
            entries(&[("fresh", false)]),
            DirCause::Nav,
            5,
        );

        assert_eq!(app.explorer.root, PathBuf::from("/fresh"));
        // "/fresh" has a parent ("/"), so a synthetic ".." row leads the list.
        let mut expected = entries(&[("fresh", false)]);
        expected.insert(
            0,
            DirEntry {
                name: "..".to_string(),
                path: PathBuf::from("/"),
                is_dir: true,
            },
        );
        assert_eq!(app.explorer.entries, expected);
    }

    #[test]
    fn initial_root_falls_back_to_app_root_for_a_pathless_doc() {
        let mut app = app();
        app.set_root(PathBuf::from("/workspace/root"));
        assert_eq!(initial_root(&app), PathBuf::from("/workspace/root"));
    }

    #[test]
    fn initial_root_falls_back_to_dot_when_app_root_is_also_unresolved() {
        // `initial_root` computes the literal `"."` fallback and then
        // resolves it through `app.vfs`; against `Mem` (WP1.S6) that
        // lexically normalizes to the synthetic root `"/"` rather than
        // staying identity, the same way `Disk::resolve` would canonicalize
        // `"."` to an absolute path rather than leaving it literal.
        let app = app();
        assert_eq!(initial_root(&app), PathBuf::from("/"));
    }

    #[test]
    fn initial_root_prefers_the_active_doc_directory_over_app_root() {
        let mut app = App::new(
            Buffer::new("hello"),
            Some(PathBuf::from("/doc/dir/note.md")),
            Arc::new(Mem::new()),
            None,
        );
        app.set_root(PathBuf::from("/workspace/root"));
        assert_eq!(initial_root(&app), PathBuf::from("/doc/dir"));
    }

    #[test]
    fn truncate_root_keeps_the_tail_behind_a_leading_ellipsis() {
        let root = Path::new("/very/deeply/nested/project/src/components");
        let truncated = truncate_root(root, 20);
        assert_eq!(crate::width::display_width(&truncated), 20);
        assert!(truncated.starts_with('\u{2026}'));
        assert!(truncated.ends_with("components"));
    }

    /// A CJK-heavy root must fit the CELL budget, not merely a per-`char`
    /// count: each ideograph is 2 cells, so a naive per-`char` fit check
    /// would pass twice as much text as the row can actually hold.
    #[test]
    fn truncate_root_respects_cell_width_for_cjk_components() {
        let root = Path::new("/\u{4e2d}\u{6587}/\u{4e2d}\u{6587}/\u{4e2d}\u{6587}");
        let truncated = truncate_root(root, 8);
        assert!(crate::width::display_width(&truncated) <= 8);
    }

    #[test]
    fn truncate_root_returns_the_path_unchanged_when_it_fits() {
        let root = Path::new("/short");
        assert_eq!(truncate_root(root, 80), "/short");
    }
}
