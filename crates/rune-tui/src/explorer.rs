//! The Explorer pane: a `Vfs::read_dir`-backed file/directory list (plan
//! WP4.S3), navigable via `listnav::List` and its own binding table.
//! `Pane::Explorer`-focused key handling lives here (`handle_key`, called
//! from `app::handle_key`'s stage-3 dispatch); row layout lives here too
//! (`draw`, delegated to from `render.rs::draw_left_pane` — the bordered
//! `Block`/focus-colored border stays render.rs's job, plan WP4.S6).
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
use crate::keymap::{Binding, KeyCode, KeyInput, KeyOutcome, KeyPattern, Mods, resolve_in};
use crate::width::truncate_tail_to_width;
use crate::listnav;
use crate::runtime::{DirCause, Effects, load_dir_cmd};
use crate::workspace;

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
}

impl Default for Explorer {
    fn default() -> Explorer {
        Explorer {
            root: PathBuf::new(),
            entries: Vec::new(),
            nav: listnav::List { cursor: 0, top: 0 },
            loading: false,
            request_generation: 0,
        }
    }
}

/// The Explorer's own commands (plan WP4.S3) — resolved via `EXPLORER_
/// BINDINGS`, mirroring `keymap::GlobalCommand`'s shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplorerCommand {
    Up,
    Down,
    Top,
    Bottom,
    Open,
    ParentDir,
}

/// Arrow keys move one entry; Home/End jump to the ends; Enter opens the
/// selected entry (a file activates it, a directory navigates into it);
/// Backspace navigates to the parent of the CURRENT root (not the selected
/// entry) — mirroring Go filetree's `..`-less parent-dir chord.
pub const EXPLORER_BINDINGS: &[Binding<ExplorerCommand>] = &[
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, Mods::NONE)],
        cmd: ExplorerCommand::Up,
        help: "up",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, Mods::NONE)],
        cmd: ExplorerCommand::Down,
        help: "down",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Home, Mods::NONE)],
        cmd: ExplorerCommand::Top,
        help: "top",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::End, Mods::NONE)],
        cmd: ExplorerCommand::Bottom,
        help: "bottom",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Enter, Mods::NONE)],
        cmd: ExplorerCommand::Open,
        help: "open",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Backspace, Mods::NONE)],
        cmd: ExplorerCommand::ParentDir,
        help: "up dir",
        when: "",
    },
];

/// The Explorer's starting root the first time it's ever shown (plan
/// WP4.S4's "`^x` triggers the initial load"): the active document's own
/// directory takes priority (deliberate, and different from Go, which
/// always roots the tree at the workspace root) — a pathless (draft)
/// session falls back to `app.root` (the workspace root discovered at
/// startup), and only when THAT is also unresolved (still empty) does this
/// fall back to the literal `"."`. Resolved through `app.vfs` (§1.4.9) so
/// `Disk` canonicalizes it exactly like every other filesystem entry point,
/// and `Mem` (tests) returns it unchanged (`Vfs::resolve`'s identity
/// behavior there).
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

/// Stage 3 of the four-stage key pipeline (plan Context, decision 8) when
/// `app.focus == Pane::Explorer`. `effects` is needed (unlike the plan's
/// literal `handle_key(app, key) -> KeyOutcome` sketch) because `Open`/
/// `ParentDir` must enqueue a `ReadDir` `Cmd` — a Vfs read can never run
/// inline in `update` (§5.4) — the same reason `app::handle_editor_key`
/// this mirrors already threads `effects` through for `Save`/clipboard.
pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    let Some(cmd) = resolve_in(EXPLORER_BINDINGS, key) else {
        return KeyOutcome::Ignored;
    };
    match cmd {
        ExplorerCommand::Up => move_selection(app, -1),
        ExplorerCommand::Down => move_selection(app, 1),
        ExplorerCommand::Top => {
            app.explorer.nav.first();
            ensure_visible(app);
        }
        ExplorerCommand::Bottom => {
            let len = app.explorer.entries.len();
            app.explorer.nav.last(len);
            ensure_visible(app);
        }
        ExplorerCommand::Open => open_selected(app, effects),
        ExplorerCommand::ParentDir => go_to_parent(app, effects),
    }
    KeyOutcome::Consumed
}

fn move_selection(app: &mut App, delta: isize) {
    let len = app.explorer.entries.len();
    app.explorer.nav.move_by(delta, len);
    ensure_visible(app);
}

/// Scrolls the Explorer's window to keep the cursor visible (plan WP4.S3:
/// "follow, margin = min(4, visible/4) like Go filetree" — Go's own
/// `ensureVisible`, jump
/// buffer 0 since Go's own `Follow` call has no jump argument either).
fn ensure_visible(app: &mut App) {
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

/// Opens the currently selected entry: a file activates it through
/// `workspace::open_path`; a directory issues a `ReadDir` `Cmd` navigating
/// the Explorer into it (plan WP4.S3: "Open on a file → workspace::
/// open_path; Open on a dir → dir load Cmd for the new root"). The
/// directory branch resolves the candidate root through `app.vfs.resolve`
/// first (§1.4.9), same as `initial_root`/`open_path` already do — a plain
/// `join` would let an unresolved (e.g. symlinked) path become the
/// Explorer's new root, unlike every other root-changing path in this
/// module. Falls back to the unresolved path on a `resolve` error, mirroring
/// `workspace::open_path`'s own `unwrap_or_else` fallback (Prime Directive:
/// a resolve failure must never just strand the user mid-navigation).
fn open_selected(app: &mut App, effects: &mut Effects) {
    let Some((name, is_dir)) = app
        .explorer
        .entries
        .get(app.explorer.nav.cursor)
        .map(|e| (e.name.clone(), e.is_dir))
    else {
        return;
    };
    let target = app.explorer.root.join(&name);
    if is_dir {
        let resolved = app.vfs.resolve(&target).unwrap_or_else(|_| target.clone());
        request_dir(app, resolved, effects);
    } else {
        let _ = workspace::open_path(app, &target);
    }
}

/// Backspace navigates to the CURRENT root's own parent — a no-op at a
/// filesystem root (`Path::parent` returns `None`), never a Cmd for a
/// nonexistent target. Resolved through `app.vfs.resolve` before use (see
/// `open_selected`'s docs) — a plain `Path::parent` is pure path arithmetic
/// that never consults the filesystem, unlike `initial_root`'s own root
/// resolution.
fn go_to_parent(app: &mut App, effects: &mut Effects) {
    let Some(parent) = app.explorer.root.parent() else {
        return;
    };
    let parent = parent.to_path_buf();
    let resolved = app.vfs.resolve(&parent).unwrap_or_else(|_| parent.clone());
    request_dir(app, resolved, effects);
}

fn request_dir(app: &mut App, root: PathBuf, effects: &mut Effects) {
    app.explorer.loading = true;
    app.explorer.request_generation = app.explorer.request_generation.wrapping_add(1);
    let generation = app.explorer.request_generation;
    let vfs = Arc::clone(&app.vfs);
    effects
        .cmds
        .push(load_dir_cmd(vfs, root, DirCause::Nav, generation));
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

/// Reacts to `Msg::DirLoaded` (plan WP4.S4), routed from `app::update_
/// inner`. A `generation` that no longer matches `app.explorer.request_
/// generation` is a reply to a SUPERSEDED request (a later `ReadDir` was
/// already issued — `request_dir`/the initial `^x` load bump the
/// generation at every issue site) and is ignored outright, never adopted
/// over whatever a newer, still-in-flight (or already-landed) request
/// produced. `Nav` always adopts the new root/entries and resets the cursor
/// to the top; `Refresh` keeps the currently selected entry selected BY
/// NAME when it's still present in the new listing (falling back to the
/// top otherwise), and is the shape a later fsnotify-driven reload would
/// use — no production caller constructs `DirCause::Refresh` yet (plan Out
/// of scope: "fsnotify/vfs-watch"), but the branch is exercised directly by
/// `tests/explorer.rs`.
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
    app.explorer.nav.cursor = preserve_name
        .and_then(|name| app.explorer.entries.iter().position(|e| e.name == name))
        .unwrap_or(0);
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
        assert_eq!(app.explorer.entries.len(), 2);
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
        app.explorer.nav.cursor = 2; // "c"

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
        app.explorer.nav.cursor = 1; // "gone"

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
        assert_eq!(app.explorer.entries, entries(&[("fresh", false)]));
    }

    #[test]
    fn up_and_down_clamp_at_the_list_bounds() {
        let mut app = app();
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("b", false), ("c", false)]),
            DirCause::Nav,
            0,
        );
        let mut effects = Effects::default();

        let up = KeyInput {
            code: KeyCode::Up,
            mods: Mods::NONE,
        };
        assert_eq!(handle_key(&mut app, up, &mut effects), KeyOutcome::Consumed);
        assert_eq!(app.explorer.nav.cursor, 0, "clamped at the top");

        let down = KeyInput {
            code: KeyCode::Down,
            mods: Mods::NONE,
        };
        for _ in 0..10 {
            assert_eq!(
                handle_key(&mut app, down, &mut effects),
                KeyOutcome::Consumed
            );
        }
        assert_eq!(app.explorer.nav.cursor, 2, "clamped at the bottom");
    }

    #[test]
    fn initial_root_falls_back_to_app_root_for_a_pathless_doc() {
        let mut app = app();
        app.set_root(PathBuf::from("/workspace/root"));
        assert_eq!(initial_root(&app), PathBuf::from("/workspace/root"));
    }

    #[test]
    fn initial_root_falls_back_to_dot_when_app_root_is_also_unresolved() {
        let app = app();
        assert_eq!(initial_root(&app), PathBuf::from("."));
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

    /// A CJK-heavy root must fit the CELL budget, not merely the char
    /// count: each ideograph is 2 cells, so a naive `chars().count()` fit
    /// check would pass twice as much text as the row can actually hold.
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
