use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

use crate::app::App;
use crate::breadcrumb_layout::{build_controls, build_crumb, crumb_parts, spans_width};
use crate::width::display_width;

const CONTROLS_LEAD: &str = "── ";
const TAIL: &str = "──╯";

const CORNER_WIDTH: usize = 1;
const TAIL_WIDTH: usize = 3;
const MIN_DASH: usize = 1;

const CRUMB_PADDING: usize = 2;

// `render::draw` must already have painted the center `Block` over `block`
// before this runs, or these cells get painted over again.
pub fn overlay(app: &App, block: Rect, focused: bool, frame: &mut Frame) {
    if block.height < 2 || block.width == 0 {
        return;
    }
    let width = block.width as usize;

    let fixed = CORNER_WIDTH + MIN_DASH + TAIL_WIDTH;
    let controls_padding = display_width(CONTROLS_LEAD) + 1;

    let controls = build_controls(&app.nav_history, &app.theme);
    let controls_block = spans_width(&controls) + controls_padding;
    let controls = (!controls.is_empty() && controls_block + fixed <= width).then_some(controls);
    let controls_block = controls.as_ref().map_or(0, |_| controls_block);

    let crumb = crumb_spans(app, width.saturating_sub(controls_block))
        .filter(|spans| spans_width(spans) + CRUMB_PADDING + controls_block + fixed <= width);
    let crumb_block = crumb
        .as_ref()
        .map_or(0, |spans| spans_width(spans) + CRUMB_PADDING);

    if controls.is_none() && crumb.is_none() {
        return;
    }
    let dash = width - CORNER_WIDTH - controls_block - crumb_block - TAIL_WIDTH;

    let border_style = if focused {
        app.theme.chrome.active_border
    } else {
        app.theme.chrome.inactive_border
    };
    let plain = Style::new();

    let y = block.y + block.height - 1;
    let buf = frame.buffer_mut();
    let mut x = block.x;

    put(buf, &mut x, y, "╰", border_style);
    if let Some(controls) = &controls {
        for cluster in CONTROLS_LEAD.graphemes(true) {
            put(buf, &mut x, y, cluster, border_style);
        }
        put_spans(buf, &mut x, y, controls);
        put(buf, &mut x, y, " ", plain);
    }
    for _ in 0..dash {
        put(buf, &mut x, y, "─", border_style);
    }
    if let Some(crumb) = &crumb {
        put(buf, &mut x, y, " ", plain);
        put_spans(buf, &mut x, y, crumb);
        put(buf, &mut x, y, " ", plain);
    }
    for cluster in TAIL.graphemes(true) {
        put(buf, &mut x, y, cluster, border_style);
    }
}

fn crumb_spans(app: &App, budget: usize) -> Option<Vec<Span<'static>>> {
    let path = app.shown_doc().resolved_path()?;
    let parts = crumb_parts(path, app.root.as_deref());
    if parts.is_empty() {
        return None;
    }
    Some(build_crumb(&parts, budget, &app.theme))
}

fn put_spans(buf: &mut ratatui::buffer::Buffer, x: &mut u16, y: u16, spans: &[Span<'_>]) {
    for span in spans {
        for cluster in span.content.graphemes(true) {
            put(buf, x, y, cluster, span.style);
        }
    }
}

/// Writes one grapheme cluster at `(*x, y)` and advances `*x` by its
/// display width, resetting every continuation cell a wide cluster covers
/// instead of leaving whatever the buffer held there before: ratatui's own
/// buffer diffing recomputes width from the leading cell and never reads a
/// continuation cell's content, so a stale glyph left behind there would
/// still show up as a real on-screen artifact.
fn put(buf: &mut ratatui::buffer::Buffer, x: &mut u16, y: u16, cluster: &str, style: Style) {
    let mut cells = Vec::new();
    let mut visual_col = 0usize;
    crate::render::push_grapheme_cells(&mut cells, &mut visual_col, cluster, None, style);
    for cell in &cells {
        if let Some(target) = buf.cell_mut((*x, y)) {
            target.set_symbol(&cell.text);
            target.set_style(cell.style);
        }
        let width = u16::from(cell.width);
        for dx in 1..width {
            if let Some(cont) = buf.cell_mut((x.saturating_add(dx), y)) {
                cont.reset();
            }
        }
        *x = x.saturating_add(width);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::testgrid;
    use rune_core::buffer::Buffer;
    use rune_syntax::wrap::grapheme_width;
    use rune_vfs::Mem;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn app_for(content: &str, path: Option<&str>) -> App {
        let vfs = Arc::new(Mem::new());
        let launch = path.map(|path| {
            crate::resolved::ResolvedPath::resolve(vfs.as_ref(), &PathBuf::from(path))
                .expect("the launch path resolves")
        });
        App::new(Buffer::new(content), launch, vfs, None)
    }

    fn overlay_bottom_row(app: &App, width: u16, height: u16, focused: bool) -> String {
        let buf = testgrid::draw_with(width, height, |frame| {
            let block = Rect::new(0, 0, width, height);
            overlay(app, block, focused, frame)
        });
        let mut s = String::new();
        // Walk by each symbol's own display width, mirroring ratatui's own
        // diffing: a wide glyph's continuation cell is blank and no real
        // terminal ever reads it, so counting it here would double count.
        let mut x = 0u16;
        while x < width {
            let Some(cell) = buf.cell((x, height - 1)) else {
                break;
            };
            let sym = cell.symbol();
            s.push_str(sym);
            x = x.saturating_add(grapheme_width(sym).max(1) as u16);
        }
        s
    }

    #[test]
    fn overlay_relativizes_against_app_root_end_to_end() {
        let mut app = app_for("hello", Some("/Users/xiii/vault/notes/note.md"));
        app.set_root(PathBuf::from("/Users/xiii/vault"));
        let row = overlay_bottom_row(&app, 60, 3, true);
        assert!(row.contains("vault/notes › note.md"));
        assert!(!row.contains("Users"));
    }

    #[test]
    fn the_crumb_is_root_relative_without_the_explorer_ever_being_shown() {
        let mut app = app_for("hello", Some("/Users/xiii/vault/notes/note.md"));
        app.set_root(PathBuf::from("/Users/xiii/vault"));
        assert!(
            !app.splits.left.is_shown(),
            "the Explorer pane must not be shown"
        );

        let row = overlay_bottom_row(&app, 60, 3, true);
        assert!(
            row.contains("vault/notes › note.md"),
            "expected a root-relative crumb:\n{row}"
        );
        assert!(!row.contains("Users"), "the path above root must be cut");
    }

    #[test]
    fn a_sibling_sharing_the_root_name_prefix_is_not_relativized() {
        let mut app = app_for("hello", Some("/a/vault2/notes.md"));
        app.set_root(PathBuf::from("/a/vault"));

        let row = overlay_bottom_row(&app, 60, 3, true);
        assert!(
            row.contains("a/vault2 › notes.md"),
            "expected the absolute-path fallback:\n{row}"
        );
    }

    // An independent width oracle: computed straight from `unicode_width`/
    // `unicode_segmentation`, never via this module's own `display_width`,
    // so a regression in the production chokepoint can't pass a test that
    // merely re-invokes it.
    fn oracle_cell_width(s: &str) -> usize {
        s.graphemes(true)
            .map(|g| {
                g.chars()
                    .filter_map(unicode_width::UnicodeWidthChar::width)
                    .max()
                    .unwrap_or(0)
                    .max(1)
            })
            .sum()
    }

    #[test]
    fn renders_the_exact_row_at_a_known_width() {
        let app = app_for("hello", Some("/a/b/note.md"));
        let row = overlay_bottom_row(&app, 60, 3, true);
        assert_eq!(
            row,
            format!(
                "╰── ^[ back  ^] forward {} a/b › note.md ──╯",
                "─".repeat(18)
            )
        );
    }

    #[test]
    fn a_too_long_path_is_truncated_with_an_ellipsis_prefix() {
        const W: u16 = 39;
        let app = app_for("hello", Some("/alpha/bravo/charlie/delta/note.md"));
        let row = overlay_bottom_row(&app, W, 3, true);
        assert_eq!(row, "╰── ^[ back  ^] forward ─ …/note.md ──╯");
        assert_eq!(
            oracle_cell_width(&row),
            W as usize,
            "the row must fill its width"
        );
    }

    // macOS normalizes on-disk file names to NFD, so "café.md" arrives as
    // "cafe" + combining acute, not the precomposed character.
    #[test]
    fn an_nfd_accent_rides_along_in_its_base_letters_cell() {
        const W: u16 = 50;
        let nfd_name = "cafe\u{0301}.md"; // "café.md", e + combining acute
        let app = app_for("hello", Some(&format!("/a/{nfd_name}")));
        let row = overlay_bottom_row(&app, W, 3, true);
        assert!(
            row.contains(nfd_name),
            "expected the NFD name verbatim in the row:\n{row}"
        );
        assert_eq!(
            oracle_cell_width(&row),
            W as usize,
            "the row must still fill its width with the accent in one cell"
        );
    }

    #[test]
    fn a_zwj_emoji_family_component_occupies_one_cell_per_cluster() {
        const W: u16 = 50;
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}"; // 👨‍👩‍👧
        let app = app_for("hello", Some(&format!("/a/{family}/note.md")));
        let row = overlay_bottom_row(&app, W, 3, true);
        assert_eq!(
            oracle_cell_width(&row),
            W as usize,
            "the row must fill its width with the family as one wide cluster"
        );
    }

    #[test]
    fn a_control_byte_in_a_path_component_never_reaches_a_raw_cell_symbol() {
        let malicious = "\u{1b}]0;pwned\u{7}";
        let app = app_for("hello", Some(&format!("/a/{malicious}")));
        let row = overlay_bottom_row(&app, 60, 3, true);
        assert!(
            !row.contains('\u{1b}') && !row.contains('\u{7}'),
            "raw ESC/BEL bytes must never land in a rendered cell:\n{row:?}"
        );
        assert!(
            row.contains('\u{241b}') && row.contains('\u{2407}'),
            "ESC/BEL must render as their control-picture placeholders:\n{row:?}"
        );
    }

    #[test]
    fn wide_path_components_keep_the_corner_in_the_last_column() {
        const W: u16 = 50;
        let app = app_for("hello", Some("/a/日本語/note.md"));
        let buf = testgrid::draw_with(W, 3, |frame| {
            overlay(&app, Rect::new(0, 0, W, 3), true, frame)
        });
        let sym = |x: u16| {
            buf.cell((x, 2))
                .map(|c| c.symbol().to_string())
                .unwrap_or_default()
        };
        assert_eq!(
            (sym(W - 3), sym(W - 2), sym(W - 1)),
            ("─".to_string(), "─".to_string(), "╯".to_string()),
            "the bottom-right corner must sit in the last column"
        );
    }

    fn blank_row(width: u16) -> String {
        let buf = testgrid::draw_with(width, 3, |_frame| {});
        let mut expected = String::new();
        for x in 0..width {
            if let Some(cell) = buf.cell((x, 2)) {
                expected.push_str(cell.symbol());
            }
        }
        expected
    }

    fn assert_control_style(app: &App, x: u16, expected: Style) {
        const W: u16 = 60;
        let buf = testgrid::draw_with(W, 3, |frame| {
            overlay(app, Rect::new(0, 0, W, 3), true, frame)
        });
        let style = buf
            .cell((x, 2))
            .map(ratatui::buffer::Cell::style)
            .unwrap_or_default();
        assert_eq!(style.fg, expected.fg);
        assert_eq!(style.add_modifier, expected.add_modifier);
        assert_eq!(
            style.bg,
            Some(ratatui::style::Color::Reset),
            "the footer background must not ride onto the border row"
        );
    }

    /// `╰── ^[ back  ^] forward`: the keystrokes start at columns 4 and 13,
    /// their words at columns 7 and 16.
    const BACK_GLYPH_X: u16 = 4;
    const BACK_WORD_X: u16 = 7;
    const FORWARD_GLYPH_X: u16 = 13;
    const FORWARD_WORD_X: u16 = 16;

    fn app_with_one_earlier_place() -> App {
        let mut app = app_for(&"line\n".repeat(40), Some("/a/b/note.md"));
        let id = app.active;
        app.active_doc_mut().cursors = rune_core::cursor::CursorSet::new(150);
        crate::navhistory::observe_jump(&mut app, id, 0);
        app
    }

    #[test]
    fn bails_out_and_leaves_the_row_untouched_at_a_tiny_width() {
        let app = app_for("hello", Some("/a/b/note.md"));
        let row = overlay_bottom_row(&app, 5, 3, true);
        assert_eq!(
            row,
            blank_row(5),
            "bail-out must leave the row exactly as-is"
        );
    }

    #[test]
    fn a_draft_renders_the_controls_without_a_crumb() {
        let app = app_for("hello", None);
        let row = overlay_bottom_row(&app, 40, 3, true);
        assert_eq!(row, format!("╰── ^[ back  ^] forward {}╯", "─".repeat(15)));
    }

    #[test]
    fn both_controls_are_dim_in_a_fresh_session() {
        let app = app_for("hello", Some("/a/b/note.md"));
        let dim = app.theme.chrome.footer_key_inactive;
        assert!(!app.nav_history.can_back());
        assert_control_style(&app, BACK_GLYPH_X, dim);
        assert_control_style(&app, FORWARD_GLYPH_X, dim);
    }

    #[test]
    fn the_words_keep_the_hint_colour_whatever_the_keystroke_does() {
        let app = app_with_one_earlier_place();
        let hint = app.theme.chrome.footer_hint;
        assert_control_style(&app, BACK_WORD_X, hint);
        assert_control_style(&app, FORWARD_WORD_X, hint);
    }

    #[test]
    fn back_lights_up_once_a_place_is_recorded() {
        let app = app_with_one_earlier_place();
        assert_control_style(&app, BACK_GLYPH_X, app.theme.chrome.footer_key);
        assert_control_style(&app, FORWARD_GLYPH_X, app.theme.chrome.footer_key_inactive);
    }

    #[test]
    fn forward_lights_up_after_stepping_back() {
        let mut app = app_with_one_earlier_place();
        crate::navhistory::back(&mut app, &mut crate::runtime::Effects::default());

        assert_control_style(&app, FORWARD_GLYPH_X, app.theme.chrome.footer_key);
        assert_control_style(&app, BACK_GLYPH_X, app.theme.chrome.footer_key_inactive);
    }

    #[test]
    fn the_crumb_is_dropped_before_the_controls() {
        let app = app_for("hello", Some("/a/b/note.md"));
        let row = overlay_bottom_row(&app, 34, 3, true);
        assert_eq!(row, format!("╰── ^[ back  ^] forward {}╯", "─".repeat(9)));
    }

    #[test]
    fn the_crumb_alone_survives_below_the_controls_width() {
        let app = app_for("hello", Some("/a/b/note.md"));
        assert_eq!(
            overlay_bottom_row(&app, 24, 3, true),
            format!("╰{} a/b › note.md ──╯", "─".repeat(5))
        );
    }

    #[test]
    fn neither_the_controls_nor_a_name_less_crumb_leaves_a_blank_row() {
        let app = app_for("hello", Some("/alpha/bravo/charlie/note.md"));
        for width in 10u16..16 {
            assert_eq!(overlay_bottom_row(&app, width, 3, true), blank_row(width));
        }
    }
}
