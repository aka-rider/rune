//! The breadcrumb: a direct port of Go's `overlayBreadcrumb`
//! (`pkg/ui/pages/workspace/workspace_view.go:430-468`) plus its
//! `buildCrumb` text (`pkg/ui/components/breadcrumb/breadcrumb.go:56-119`) —
//! plan WP4.S4, replacing the pre-WP4 `draw(app, area, frame)` (which gave
//! the breadcrumb its OWN reserved row) now that the center pane has a real
//! `Block::bordered()` (WP4.S2) to splice onto instead. `overlay` writes
//! cells directly into `frame.buffer_mut()` on the block's own BOTTOM
//! border row — the same cell-writing idiom `render::blit` uses — rather
//! than depending on ratatui's `Block` title-placement semantics, so the
//! arithmetic can match Go's exactly (the 2-dash right margin, the `+7`
//! bail-out, the `... / ` ellipsis) cell for cell.
//!
//! Render order is load-bearing (plan gotcha 16): `render::draw` must have
//! already painted the center `Block` over `block` before calling this, or
//! `overlay`'s cells get painted over again.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use std::path::Component;

use crate::app::App;
use crate::styles;
use rune_md::wrap::control_aware_width;

/// The display width of `s`, measured through the crate's ONE width
/// chokepoint (`render::segment_cells`'s own `control_aware_width`, itself
/// `rune_md::wrap`'s — see `render.rs`'s "ONE width chokepoint" note). Every
/// width in this module — the `bc` total, `build_crumb`'s per-part
/// accounting, and `put`'s column advance — goes through it, so the dash
/// fill can never be sized in one unit and drawn in another (§1.5: display
/// widths are one system, and a CJK/emoji path component makes the
/// difference visible immediately).
fn text_width(s: &str) -> usize {
    s.chars().map(control_aware_width).sum()
}

/// Go's `Padding(0, 1)`-rendered ellipsis (`st.Breadcrumb.Render("... / ")`,
/// `breadcrumb.go:90`): the padding adds one space on EACH side of the
/// already-trailing-spaced `"... / "` literal, giving `" ... /  "` — two
/// trailing spaces, not one.
const ELLIPSIS: &str = " ... /  ";

/// Go's padding-free separator (`st.BreadcrumbSep.Render(" / ")`,
/// `breadcrumb.go:87`).
const SEP: &str = " / ";

/// Splices the active document's breadcrumb onto `block`'s bottom border
/// row (`block.y + block.height - 1`), left to right: `╰` · a `─` dash
/// fill · a plain space · the crumb text · a plain space · `──╯`. Does
/// nothing (leaves whatever `render::draw`'s `Block` already painted on
/// that row) when:
/// - `block` is too small to have a distinct bottom border row of its own
///   (`block.height < 2`) or has no width at all;
/// - the active document has no `file_path` (a draft, the Help virtual
///   doc — same "renders nothing" contract the pre-WP4 `draw` had);
/// - the path has no `Normal` components at all (Go never shows a bare
///   `/`);
/// - the crumb, even at its MOST truncated (a single leaf part plus the
///   ellipsis, or fewer), still doesn't fit `block`'s width with Go's
///   `minOverhead` of 7 columns spare (`bc + 7 > block.width`).
pub fn overlay(app: &App, block: Rect, focused: bool, frame: &mut Frame) {
    if block.height < 2 || block.width == 0 {
        return;
    }
    let Some(path) = &app.active_doc().file_path else {
        return;
    };
    let parts: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return;
    }

    let segments = build_crumb(&parts, block.width as usize);

    let bc: usize = segments
        .iter()
        .map(|s| text_width(s.content.as_ref()))
        .sum();
    // Go's `minOverhead := 7` (`workspace_view.go`'s bail-out) — leaves the
    // plain border row (already painted by `render::draw`'s `Block`)
    // completely untouched rather than splicing a crumb that would collide
    // with the corner glyphs.
    if bc + 7 > block.width as usize {
        return;
    }
    let dash = block.width as usize - bc - 6;

    let border_style = if focused {
        styles::active_border()
    } else {
        styles::inactive_border()
    };
    let plain = Style::new();

    let y = block.y + block.height - 1;
    let buf = frame.buffer_mut();
    let mut x = block.x;

    put(buf, &mut x, y, '╰', border_style);
    for _ in 0..dash {
        put(buf, &mut x, y, '─', border_style);
    }
    put(buf, &mut x, y, ' ', plain);
    for span in &segments {
        for ch in span.content.chars() {
            put(buf, &mut x, y, ch, span.style);
        }
    }
    put(buf, &mut x, y, ' ', plain);
    for ch in "──╯".chars() {
        put(buf, &mut x, y, ch, border_style);
    }
}

/// A direct port of Go's `buildCrumb` (`breadcrumb.go:56-119`), minus the
/// workspace-root relativization (plan Out of scope — Rust has no
/// workspace-root concept on `App` yet; see root `TODO.md`): builds the
/// `Normal` path components right-to-left, each as `" part "` (fg
/// `SPECIAL`, Go's `Breadcrumb` style) followed by `" / "` (fg `SUBTLE`,
/// Go's `BreadcrumbSep`) for every part except the rightmost (leaf), until
/// adding the next part would overflow `max_width` by Go's 6-column
/// buffer — at which point an `ELLIPSIS` span is prepended and the loop
/// stops. Index `0` (the leftmost/root-most component) is NEVER dropped —
/// Go's `&& i > 0` guard on the truncation check.
fn build_crumb(parts: &[String], max_width: usize) -> Vec<Span<'static>> {
    let n = parts.len();
    let mut segments: Vec<Span<'static>> = Vec::with_capacity(n * 2);
    let mut current_width = 0usize;

    for (i, part) in parts.iter().enumerate().rev() {
        let part_text = format!(" {part} ");
        let part_width = text_width(part_text.as_str());
        let is_last = i == n - 1;

        let (seg_width, seg): (usize, Vec<Span<'static>>) = if is_last {
            (
                part_width,
                vec![Span::styled(part_text, Style::new().fg(styles::SPECIAL))],
            )
        } else {
            let sep_width = text_width(SEP);
            (
                part_width + sep_width,
                vec![
                    Span::styled(part_text, Style::new().fg(styles::SPECIAL)),
                    Span::styled(SEP, Style::new().fg(styles::SUBTLE)),
                ],
            )
        };

        // Go's `6`: an arbitrary buffer for the ellipsis and some
        // breathing room (`breadcrumb.go:99-100`'s own comment).
        if current_width + seg_width + 6 > max_width && i > 0 {
            segments.insert(0, Span::styled(ELLIPSIS, Style::new().fg(styles::SPECIAL)));
            return segments;
        }

        for span in seg.into_iter().rev() {
            segments.insert(0, span);
        }
        current_width += seg_width;
    }

    segments
}

/// Writes one character at `(*x, y)` and advances `*x` by that character's
/// DISPLAY width — the identical idiom `render::blit` uses
/// (`x.saturating_add(cell.width.max(1) as u16)`), and the reason this
/// module can splice into a border row at all. Advancing by 1 per `char`
/// while `overlay` sizes its dash fill in display columns would desync the
/// two the moment a path component holds a CJK/emoji glyph: the `──╯` would
/// land short of the right edge, leaving stale border cells behind it, and
/// the cell ratatui reserves after a double-width glyph would be written
/// into. Out-of-buffer writes are dropped by `cell_mut` returning `None`.
fn put(buf: &mut ratatui::buffer::Buffer, x: &mut u16, y: u16, ch: char, style: Style) {
    if let Some(cell) = buf.cell_mut((*x, y)) {
        cell.set_char(ch);
        cell.set_style(style);
    }
    *x = x.saturating_add(control_aware_width(ch).max(1) as u16);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn app_for(content: &str, path: Option<&str>) -> App {
        App::new(
            Buffer::new(content),
            path.map(PathBuf::from),
            Arc::new(Mem::new()),
            None,
        )
    }

    /// Draws `overlay` into a `height`-tall `TestBackend` and returns the
    /// bottom row's rendered symbols concatenated into one `String` — the
    /// row `overlay` actually writes to.
    fn overlay_bottom_row(app: &App, width: u16, height: u16, focused: bool) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal construction");
        terminal
            .draw(|frame| {
                let block = Rect::new(0, 0, width, height);
                overlay(app, block, focused, frame)
            })
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let mut s = String::new();
        for x in 0..width {
            if let Some(cell) = buf.cell((x, height - 1)) {
                s.push_str(cell.symbol());
            }
        }
        s
    }

    #[test]
    fn renders_the_exact_row_at_a_known_width() {
        let app = app_for("hello", Some("/a/b/note.md"));
        // parts = ["a", "b", "note.md"]; crumb = " a  /  b  /  note.md "
        // (21 wide, itself already one-space-padded on each end from its
        // own leftmost/rightmost part's Padding(0,1) — Go's `buildCrumb`).
        // `overlay` wraps that in its OWN plain leading/trailing space
        // (Go's `content := " " + crumb + " "`), so the actual row has a
        // DOUBLE space on each side of the crumb, not a single one.
        // At width 40, dash = 40 - 21 - 6 = 13.
        let row = overlay_bottom_row(&app, 40, 3, true);
        assert_eq!(
            row,
            format!("╰{}  a  /  b  /  note.md  ──╯", "─".repeat(13))
        );
    }

    #[test]
    fn wide_path_components_keep_the_corner_in_the_last_column() {
        // A CJK component is 2 display columns per `char`. `overlay` sizes
        // its dash fill in display columns, so `put` must advance in the
        // same unit: advancing 1-per-`char` would land `──╯` three columns
        // short of the right edge here, leaving stale cells behind the
        // corner (§1.5 — the two coordinate systems must not be mixed).
        const W: u16 = 40;
        let app = app_for("hello", Some("/a/日本語/note.md"));
        let backend = TestBackend::new(W, 3);
        let mut terminal = Terminal::new(backend).expect("terminal construction");
        terminal
            .draw(|frame| overlay(&app, Rect::new(0, 0, W, 3), true, frame))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
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

    #[test]
    fn bails_out_and_leaves_the_row_untouched_at_a_tiny_width() {
        let app = app_for("hello", Some("/a/b/note.md"));
        // Even the smallest possible crumb can't fit `bc + 7 <= width`
        // at width 5 — the row must come back exactly as the plain
        // `TestBackend` default (blank cells), untouched by `overlay`.
        let untouched = overlay_bottom_row(&app_for("hello", None), 5, 3, true);
        let row = overlay_bottom_row(&app, 5, 3, true);
        assert_eq!(row, untouched, "bail-out must leave the row exactly as-is");
    }

    #[test]
    fn pathless_doc_renders_nothing() {
        let app = app_for("hello", None);
        let touched = overlay_bottom_row(&app, 40, 3, true);
        let untouched_backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(untouched_backend).expect("terminal construction");
        terminal.draw(|_frame| {}).expect("draw");
        let buf = terminal.backend().buffer().clone();
        let mut expected = String::new();
        for x in 0..40 {
            if let Some(cell) = buf.cell((x, 2)) {
                expected.push_str(cell.symbol());
            }
        }
        assert_eq!(touched, expected);
    }

    #[test]
    fn total_width_identity_holds() {
        // 1 (╰) + dash + (bc + 2 surrounding spaces) + 3 (──╯) == block.width,
        // for every width from the bail-out floor up to a generous ceiling.
        for width in 30u16..80 {
            let block = Rect::new(0, 0, width, 3);
            let parts: Vec<String> = vec!["a".into(), "b".into(), "note.md".into()];
            let segments = build_crumb(&parts, width as usize);
            let bc: usize = segments
                .iter()
                .map(|s| text_width(s.content.as_ref()))
                .sum();
            if bc + 7 > block.width as usize {
                continue;
            }
            let dash = block.width as usize - bc - 6;
            assert_eq!(1 + dash + (bc + 2) + 3, block.width as usize);
        }
    }
}
