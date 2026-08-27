use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use rune_vfs::{DirEntry, FileKind, Link};

use crate::app::App;
use crate::document::DocumentId;
use crate::listnav;
use crate::pane::Pane;
use crate::runtime::{DirCause, Effects, load_dir_cmd};
use crate::width::truncate_tail_to_width;

pub struct Explorer {
    pub root: PathBuf,
    pub entries: Vec<DirEntry>,
    pub nav: listnav::List,
    pub loading: bool,
    pub request_generation: crate::generation::DirLoadGen,
    next_request_gen: crate::generation::GenCounter<crate::generation::DirLoad>,
    pub pending_reveal: Option<PathBuf>,
    pub preview: Option<DocumentId>,
    pub browsing_origin: crate::returnto::ReturnTo,
    pub preview_awaiting: Option<PathBuf>,
    pub preview_generation: crate::generation::PreviewGen,
    next_preview_gen: crate::generation::GenCounter<crate::generation::Preview>,
    pub preview_failed: Option<PathBuf>,
}

impl Explorer {
    pub(crate) fn mint_preview_generation(&mut self) -> crate::generation::PreviewGen {
        self.next_preview_gen.mint()
    }
}

impl Default for Explorer {
    fn default() -> Explorer {
        Explorer {
            root: PathBuf::new(),
            entries: Vec::new(),
            nav: listnav::List { cursor: 0, top: 0 },
            loading: false,
            request_generation: crate::generation::Generation::ZERO,
            next_request_gen: crate::generation::GenCounter::default(),
            pending_reveal: None,
            preview: None,
            browsing_origin: crate::returnto::ReturnTo::none(),
            preview_awaiting: None,
            preview_generation: crate::generation::Generation::ZERO,
            next_preview_gen: crate::generation::GenCounter::default(),
            preview_failed: None,
        }
    }
}

pub fn initial_root(app: &App) -> PathBuf {
    let base = app
        .active_doc()
        .file_path
        .as_deref()
        .and_then(Path::parent)
        .map_or_else(
            || app.root.clone().unwrap_or_else(|| PathBuf::from(".")),
            Path::to_path_buf,
        );
    app.vfs.resolve(&base).unwrap_or(base)
}

pub(crate) fn ensure_visible(app: &mut App) {
    let len = app.explorer.entries.len();
    let height = visible_rows(app);
    let margin = (height / 4).min(4);
    app.explorer.nav.follow(len, height, margin, 0);
}

fn visible_rows(app: &App) -> usize {
    let area = app.frame_area();
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

pub(crate) fn request_dir(app: &mut App, root: PathBuf, effects: &mut Effects) {
    app.explorer.loading = true;
    let generation = app.explorer.next_request_gen.mint();
    app.explorer.request_generation = generation;
    let vfs = Arc::clone(&app.vfs);
    effects
        .cmds
        .push(load_dir_cmd(vfs, root, DirCause::Nav, generation));
}

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

pub(crate) fn refresh_for(app: &mut App, path: &Path, effects: &mut Effects) {
    if app.explorer.entries.is_empty() {
        return;
    }
    let Some(parent) = path.parent() else { return };
    if parent != app.explorer.root {
        return;
    }
    app.explorer.loading = true;
    let generation = app.explorer.next_request_gen.mint();
    app.explorer.request_generation = generation;
    let vfs = Arc::clone(&app.vfs);
    let root = app.explorer.root.clone();
    effects
        .cmds
        .push(load_dir_cmd(vfs, root, DirCause::Refresh, generation));
}

pub(crate) use crate::explorer_dirload::handle_dir_loaded;

fn row_style(app: &App, entry: &DirEntry) -> Style {
    let chrome = &app.theme.chrome;
    let is_dir = entry.kind == FileKind::Dir;
    match entry.link {
        Link::No if is_dir => chrome.dir_normal,
        Link::No => chrome.file_normal,
        Link::To if is_dir => chrome.link_dir,
        Link::To => chrome.link_file,
        Link::Broken => chrome.link_broken,
    }
}

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
        let style = row_style(app, entry);
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
