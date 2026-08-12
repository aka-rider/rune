//! The Explorer pane: a `Vfs::read_dir`-backed file/directory list,
//! navigable via `listnav::List`. `Pane::Explorer`-focused key handling
//! lives in the sibling `explorer_keys` module, its `Msg::DirLoaded`
//! reaction in `explorer_dirload`, and `reveal` in `explorer_reveal` — all
//! split out to stay under the 500-line budget; row layout lives here (`draw`, delegated to from
//! `render.rs::draw_left_pane` — the bordered `Block`/focus-colored border
//! stays render.rs's job).
//!
//! Directory loading is a boundary `Msg`, never inline: crossing a thread
//! to read the filesystem can't happen mid-`update`, so `runtime::
//! load_dir_cmd` runs `vfs.read_dir` off-thread and replies with `Msg::
//! DirLoaded`, routed from `app::update_inner` to `handle_dir_loaded`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use rune_vfs::{DirEntry, FileKind};

use crate::app::App;
use crate::document::DocumentId;
use crate::listnav;
use crate::pane::Pane;
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
    /// generation token `Msg::DirLoaded` carries back. Two in-flight
    /// `ReadDir` Cmds can land out of order (e.g. Backspace to a slow parent
    /// directory, then immediately Enter into a fast child one): without
    /// this, the OLDER reply could overwrite the newer listing. Mirrors
    /// `DocDb::snapshot_generation`'s debounce-token pattern (`db.rs`) —
    /// bump in place, compare on receipt, ignore a stale one.
    pub request_generation: u32,
    /// The file `explorer_reveal::reveal` wants the cursor on, set when it
    /// issues a re-rooting `ReadDir`, consumed by `explorer_dirload::
    /// handle_dir_loaded`. Guarded by `request_generation` exactly like
    /// every other in-flight `ReadDir`: a stale reply is dropped before it
    /// ever reaches this field, so it can never consume a reveal meant for
    /// the request that superseded it.
    pub pending_reveal: Option<PathBuf>,
    /// The minted `ReadOnly::Preview` document currently occupying a slot in
    /// `documents.order()`, if the cursor is sitting on a file that isn't already
    /// open as a real tab — `explorer_preview`'s own state, kept here
    /// because it's the Explorer's cursor position that drives it. At most
    /// one preview exists at a time: moving the cursor onto a different
    /// unopened file replaces this SAME document's content and path rather
    /// than minting a second one, which is what keeps arrowing through N
    /// files at exactly one extra tab. `None` whenever the cursor sits on a
    /// directory, an already-open document, or nothing has been previewed
    /// yet this session.
    pub preview: Option<DocumentId>,
    /// The document that was active right before this browsing session's
    /// preview was first minted (`explorer_preview::apply_loaded`'s own
    /// mint branch, captured before its `switch_to` moves `app.active`
    /// off it) — where discarding the preview restores the user to,
    /// rather than an arbitrary tab. `None` exactly when `preview` is.
    pub preview_return_to: Option<DocumentId>,
    /// Paths `explorer_preview` has asked the `Vfs` to read but hasn't
    /// heard back from yet — a request is removed the moment ITS OWN reply
    /// lands, whether that reply is adopted or found stale, so this can
    /// never grow unbounded across a long arrow-key session.
    pub preview_awaiting: HashSet<PathBuf>,
    /// The path the live preview placeholder currently names, if the last
    /// read for it failed — `request_preview` treats this exactly like
    /// `already_showing`, so a failed read is not retried on every step
    /// while the cursor sits still, but IS retried once the cursor leaves
    /// and comes back. Cleared alongside `preview` in
    /// `remove_preview_document` and by a later successful `apply_loaded`.
    pub preview_failed: Option<PathBuf>,
}

impl Default for Explorer {
    fn default() -> Explorer {
        Explorer {
            root: PathBuf::new(),
            entries: Vec::new(),
            nav: listnav::List { cursor: 0, top: 0 },
            loading: false,
            request_generation: 0,
            pending_reveal: None,
            preview: None,
            preview_return_to: None,
            preview_awaiting: HashSet::new(),
            preview_failed: None,
        }
    }
}

/// The Explorer's starting root the first time it's ever shown (plan
/// WP4.S4's "`^x` triggers the initial load"): the active document's own
/// directory takes priority (a deliberate choice, rather than always
/// rooting the tree at the workspace root) — a pathless (draft)
/// session falls back to `app.root` (the workspace root discovered at
/// startup), and only when THAT is also unresolved (still empty) does this
/// fall back to the literal `"."`. Resolved through `app.vfs` so
/// `Disk` canonicalizes it exactly like every other filesystem entry point;
/// `Mem` (tests) normalizes it lexically to its own synthetic root (`Mem`
/// has no real cwd to canonicalize `"."` against).
pub fn initial_root(app: &App) -> PathBuf {
    let base = app
        .active_doc()
        .file_path
        .as_deref()
        .and_then(Path::parent)
        .map_or_else(
            || {
                if app.root.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    app.root.clone()
                }
            },
            Path::to_path_buf,
        );
    app.vfs.resolve(&base).unwrap_or(base)
}

/// Scrolls the Explorer's window to keep the cursor visible (plan WP4.S3:
/// follow, margin = min(4, visible/4), jump buffer 0 since this call has no
/// jump argument).
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
    entry_rows(crate::layout::geometry(area, app).explorer_inner).max(1)
}

pub(crate) fn entry_rows(rect: Rect) -> usize {
    (rect.height as usize).saturating_sub(1)
}

pub(crate) fn entry_at(app: &App, rect: Rect, row: u16) -> Option<usize> {
    if row == 0 || row >= rect.height {
        return None;
    }
    let window = app
        .explorer
        .nav
        .window(app.explorer.entries.len(), entry_rows(rect));
    let index = window.start.saturating_add(row as usize).saturating_sub(1);
    (index < window.end).then_some(index)
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
/// an incorrect state. A section this frame's `LayoutMode` doesn't paint —
/// hidden outright, or squeezed out by a frame too narrow to fit it even
/// though its `Split` still says shown — is left alone: nothing is on
/// screen to fill.
pub fn ensure_loaded(app: &mut App, effects: &mut Effects) {
    let messages_open = crate::messages::is_open(app);
    if app
        .layout_mode()
        .focusable(Pane::Explorer, messages_open)
        .is_none()
        || !app.explorer.entries.is_empty()
        || app.explorer.loading
    {
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

/// `handle_dir_loaded` (reacts to `Msg::DirLoaded`) lives in the sibling
/// `explorer_dirload` module (500-line budget) — re-exported here so
/// every existing `explorer::handle_dir_loaded` call site keeps working
/// unaware it moved.
pub(crate) use crate::explorer_dirload::handle_dir_loaded;

/// Draws the Explorer's content into `area` — the block's INNER rect
/// (border already rendered by `render.rs::draw_left_pane`, plan WP4.S6):
/// row 0 is the root path (truncated with a leading `…` when it doesn't
/// fit, `theme.chrome.pane_title`); the remaining rows are the `listnav`-
/// windowed entry slice, laid out `[› prefix][icon column, Nerd tier
/// only][name][/ if dir]`. A directory's name is `theme.chrome.dir_normal`
/// (bold blue), a file's is `file_normal`; the icon is monochrome, painted
/// in the row's own style rather than a colour of its own. The `›` prefix
/// is always on, so the cursor row's position survives an unfocused pane
/// or a terminal that drops backgrounds; the cursor row's own background
/// bar is painted separately, after `render_widget`, and only while
/// `Pane::Explorer` holds focus.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    if area.height == 0 {
        return;
    }
    let mut lines = Vec::with_capacity(area.height as usize);
    lines.push(Line::from(Span::styled(
        truncate_root(&app.explorer.root, area.width as usize),
        app.theme.chrome.pane_title,
    )));

    let window = app
        .explorer
        .nav
        .window(app.explorer.entries.len(), entry_rows(area));
    let start = window.start;
    let visible = app.explorer.entries.get(window).unwrap_or(&[]);
    let mut cursor_row: Option<u16> = None;
    for (i, entry) in visible.iter().enumerate() {
        let idx = start + i;
        let selected = idx == app.explorer.nav.cursor;
        if selected {
            cursor_row = Some(lines.len() as u16);
        }
        let prefix = if selected { "\u{203a} " } else { "  " };
        let is_dir = entry.kind == FileKind::Dir;
        let suffix = if is_dir { "/" } else { "" };
        let style = if is_dir {
            app.theme.chrome.dir_normal
        } else {
            app.theme.chrome.file_normal
        };
        let icon = crate::fileicons::icon(app.icon_tier, entry)
            .map(|glyph| format!("{glyph} "))
            .unwrap_or_default();
        lines.push(Line::from(Span::styled(
            format!("{prefix}{icon}{}{suffix}", entry.name),
            style,
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);

    if app.focus() == Pane::Explorer
        && let Some(y) = cursor_row
    {
        let row = Rect::new(area.x, area.y + y, area.width, 1);
        crate::render::rowbg::fill_row(frame, row, app.theme.chrome.row_cursor_bg);
    }
}

/// Truncates `root`'s displayed path to fit `width` terminal CELLS,
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
