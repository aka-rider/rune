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
use crate::listnav;
use crate::runtime::{DirCause, Effects, load_dir_cmd};
use crate::styles;
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
        key: KeyPattern::new(KeyCode::Up, Mods::NONE),
        cmd: ExplorerCommand::Up,
        help: "up",
    },
    Binding {
        key: KeyPattern::new(KeyCode::Down, Mods::NONE),
        cmd: ExplorerCommand::Down,
        help: "down",
    },
    Binding {
        key: KeyPattern::new(KeyCode::Home, Mods::NONE),
        cmd: ExplorerCommand::Top,
        help: "top",
    },
    Binding {
        key: KeyPattern::new(KeyCode::End, Mods::NONE),
        cmd: ExplorerCommand::Bottom,
        help: "bottom",
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, Mods::NONE),
        cmd: ExplorerCommand::Open,
        help: "open",
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, Mods::NONE),
        cmd: ExplorerCommand::ParentDir,
        help: "up dir",
    },
];

/// The Explorer's starting root the first time it's ever shown (plan
/// WP4.S4's "`^x` triggers the initial load"): the active document's own
/// directory, or the process's current directory for a pathless (draft)
/// session — resolved through `app.vfs` (§1.4.9) so `Disk` canonicalizes it
/// exactly like every other filesystem entry point, and `Mem` (tests)
/// returns it unchanged (`Vfs::resolve`'s identity behavior there).
pub fn initial_root(app: &App) -> PathBuf {
    let base = app
        .active_doc()
        .file_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
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
/// "follow, margin = min(4, visible/4) like Go filetree" —
/// `pkg/ui/components/filetree/filetree.go:64-65`'s `ensureVisible`, jump
/// buffer 0 since Go's own `Follow` call has no jump argument either).
fn ensure_visible(app: &mut App) {
    let len = app.explorer.entries.len();
    let height = visible_rows(app);
    let margin = (height / 4).min(4);
    app.explorer.nav.follow(len, height, margin, 0);
}

/// Approximates the Explorer pane's visible entry-row count for `ensure_
/// visible`'s scroll margin. `Explorer` stores no height of its own (plan
/// WP4.S3's exact field list has none), so this derives one from the
/// active document's viewport height — kept live by every `Msg::Resize`
/// (`document.rs`) — mirroring `render.rs::draw`'s own split: the main
/// area is `viewport.height + 1` (the footer's one row), the left column
/// splits it 50/50 with Open Tabs (`draw_left_pane`), each pane loses 2
/// rows to its own border and this one more to its root-path title row.
/// An approximation only (percentage-split rounding can be off by a row) —
/// harmless, since `listnav::List::window` independently clamps to the
/// REAL rect at render time regardless of what `top` this over/under-
/// estimated.
fn visible_rows(app: &App) -> usize {
    let main_area_height = app.active_doc().viewport.height as usize + 1;
    let pane_height = (main_area_height / 2).saturating_sub(2);
    pane_height.saturating_sub(1).max(1)
}

/// Opens the currently selected entry: a file activates it through
/// `workspace::open_path`; a directory issues a `ReadDir` `Cmd` navigating
/// the Explorer into it (plan WP4.S3: "Open on a file → workspace::
/// open_path; Open on a dir → dir load Cmd for the new root").
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
        request_dir(app, target, effects);
    } else {
        workspace::open_path(app, &target);
    }
}

/// Backspace navigates to the CURRENT root's own parent — a no-op at a
/// filesystem root (`Path::parent` returns `None`), never a Cmd for a
/// nonexistent target.
fn go_to_parent(app: &mut App, effects: &mut Effects) {
    let Some(parent) = app.explorer.root.parent() else {
        return;
    };
    request_dir(app, parent.to_path_buf(), effects);
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
/// fit, `styles::pane_title`); the remaining rows are the `listnav`-
/// windowed entry slice, `>` prefixed and `styles::file_selected` on the
/// cursor row, `/` suffixed for a directory.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    if area.height == 0 {
        return;
    }
    let mut lines = Vec::with_capacity(area.height as usize);
    lines.push(Line::from(Span::styled(
        truncate_root(&app.explorer.root, area.width as usize),
        styles::pane_title(),
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
            styles::file_selected()
        } else {
            styles::file_normal()
        };
        lines.push(Line::from(Span::styled(
            format!("{prefix}{}{suffix}", entry.name),
            style,
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Truncates `root`'s displayed path to fit `width` columns, keeping the
/// TAIL and marking the cut with a leading `…` (plan WP4.S3: "root-path
/// title row truncated with leading `…`") — the tail (the directory's own
/// name and its nearest ancestors) is what a user navigating a deep tree
/// actually needs to see, not the common prefix every row would otherwise
/// share.
fn truncate_root(root: &Path, width: usize) -> String {
    let text = root.display().to_string();
    if width == 0 {
        return String::new();
    }
    let char_count = text.chars().count();
    if char_count <= width {
        return text;
    }
    let keep = width.saturating_sub(1);
    let tail: String = text.chars().skip(char_count - keep).collect();
    format!("\u{2026}{tail}")
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
    fn truncate_root_keeps_the_tail_behind_a_leading_ellipsis() {
        let root = Path::new("/very/deeply/nested/project/src/components");
        let truncated = truncate_root(root, 20);
        assert_eq!(truncated.chars().count(), 20);
        assert!(truncated.starts_with('\u{2026}'));
        assert!(truncated.ends_with("components"));
    }

    #[test]
    fn truncate_root_returns_the_path_unchanged_when_it_fits() {
        let root = Path::new("/short");
        assert_eq!(truncate_root(root, 80), "/short");
    }
}
