//! The breadcrumb: the active document's path spliced onto the center
//! pane's bottom border row — plan WP4.S4, replacing the pre-WP4
//! `draw(app, area, frame)` (which gave the breadcrumb its OWN reserved
//! row) now that the center pane has a real `Block::bordered()` (WP4.S2)
//! to splice onto instead. `overlay` writes cells directly into
//! `frame.buffer_mut()` on the block's own BOTTOM border row — the same
//! cell-writing idiom `render::blit` uses — rather than depending on
//! ratatui's `Block` title-placement semantics, so the arithmetic (the
//! 2-dash right margin, the `+7` bail-out, the `…/` ellipsis) is exact
//! cell for cell.
//!
//! The rendered shape is `dir/dir/dir › leaf`: directories run together
//! separated by a bare `/`, and a wider ` › ` sets the leaf file name off
//! from the directory chain. Parts themselves carry no padding — the one
//! space on each side of the whole crumb comes from `overlay`.
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
use rune_syntax::wrap::control_aware_width;

/// The display width of `s`, measured through the crate's ONE width
/// chokepoint (`rune_syntax::wrap`'s `control_aware_width`, the same one
/// the renderer's cell segmentation uses). Every width in this module —
/// the `bc` total, `build_crumb`'s per-part accounting, and `put`'s column
/// advance — goes through it, so the dash fill can never be sized in one
/// unit and drawn in another (§1.5: display widths are one system, and a
/// CJK/emoji path component makes the difference visible immediately).
fn text_width(s: &str) -> usize {
    s.chars().map(control_aware_width).sum()
}

/// Marks a crumb whose leading parts were dropped to fit the width. Two
/// display columns; the trailing `/` reads as "…and more directories
/// above", continuing the bare-slash directory chain that follows it.
const ELLIPSIS: &str = "…/";

/// Between two directories: a bare slash, no padding.
const SEP: &str = "/";

/// Between the LAST directory and the leaf file name only — the one place
/// the crumb breathes, so the file name reads as the subject and the
/// directory chain as its address.
const LEAF_SEP: &str = " › ";

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
    let parts = crumb_parts(path, &app.root);
    if parts.is_empty() {
        return;
    }

    let segments = build_crumb(&parts, block.width as usize, &app.theme);

    let bc: usize = segments
        .iter()
        .map(|s| text_width(s.content.as_ref()))
        .sum();
    // The 7-column minimum overhead — leaves the
    // plain border row (already painted by `render::draw`'s `Block`)
    // completely untouched rather than splicing a crumb that would collide
    // with the corner glyphs.
    if bc + 7 > block.width as usize {
        return;
    }
    let dash = block.width as usize - bc - 6;

    let border_style = if focused {
        app.theme.chrome.active_border
    } else {
        app.theme.chrome.inactive_border
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

/// Relativizes `path` against `root` (plan WP4.S6, Go's own `buildCrumb`
/// `root` argument), returning the ordered list of path components the
/// crumb renders. When `root` is non-empty and `path` is under it (`Path::
/// starts_with` compares whole components, so this can never mistake
/// `/a/vault2` for being under `/a/vault` the way a bare string-prefix
/// check would), the result is root's own base name followed by the
/// remaining components below it — e.g. root `/Users/xiii/vault`, path
/// `/Users/xiii/vault/notes/note.md` yields `["vault", "notes",
/// "note.md"]`, not `["Users", "xiii", "vault", "notes", "note.md"]`.
/// Falls back to every `Normal` component of the absolute path otherwise
/// (`root` empty — not yet resolved — or `path` outside it).
fn crumb_parts(path: &std::path::Path, root: &std::path::Path) -> Vec<String> {
    if !root.as_os_str().is_empty()
        && let Ok(remainder) = path.strip_prefix(root)
    {
        let mut parts = Vec::new();
        if let Some(name) = root.file_name() {
            parts.push(name.to_string_lossy().into_owned());
        }
        parts.extend(remainder.components().filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        }));
        return parts;
    }
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// Builds the crumb's spans right-to-left: each part verbatim (fg
/// `SPECIAL`, no padding of its own) followed — for every part except the
/// rightmost (the leaf) — by a `SUBTLE` separator span, `LEAF_SEP` when
/// the part is the last directory (index `n - 2`) and `SEP` between any
/// two directories above it. The walk stops as soon as adding the next
/// part would overflow `max_width` by the 6-column buffer, at which point
/// an `ELLIPSIS` span is prepended. Index `0` (the leftmost/root-most
/// component) is NEVER dropped — the `&& i > 0` guard on the truncation
/// check. A single-component path renders as just that leaf.
fn build_crumb(
    parts: &[String],
    max_width: usize,
    theme: &crate::theme::Theme,
) -> Vec<Span<'static>> {
    let n = parts.len();
    let mut segments: Vec<Span<'static>> = Vec::with_capacity(n * 2);
    let mut current_width = 0usize;

    for (i, part) in parts.iter().enumerate().rev() {
        let part_width = text_width(part);
        let is_last = i == n - 1;

        let (seg_width, seg): (usize, Vec<Span<'static>>) = if is_last {
            (
                part_width,
                vec![Span::styled(
                    part.clone(),
                    Style::new().fg(theme.chrome.special),
                )],
            )
        } else {
            // The part directly left of the leaf gets the wider ` › `;
            // every directory above it is joined by a bare `/`.
            let sep = if i + 2 == n { LEAF_SEP } else { SEP };
            (
                part_width + text_width(sep),
                vec![
                    Span::styled(part.clone(), Style::new().fg(theme.chrome.special)),
                    Span::styled(sep, Style::new().fg(theme.chrome.subtle)),
                ],
            )
        };

        // An arbitrary buffer for the ellipsis and some breathing room.
        if current_width + seg_width + 6 > max_width && i > 0 {
            segments.insert(
                0,
                Span::styled(ELLIPSIS, Style::new().fg(theme.chrome.special)),
            );
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
    use crate::testgrid;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn app_for(content: &str, path: Option<&str>) -> App {
        App::new(
            Buffer::new(content),
            path.map(PathBuf::from),
            Arc::new(Mem::new()),
            None,
        )
    }

    /// Draws `overlay` into a `height`-tall `TestBackend` (via the shared
    /// `testgrid::draw_with` — plan WP1: `overlay` renders a component
    /// directly into its own `Rect`, not the whole `App` through
    /// `render::draw`, so `grid`/`row` don't apply here) and returns the
    /// bottom row's rendered symbols concatenated into one `String` — the
    /// row `overlay` actually writes to.
    fn overlay_bottom_row(app: &App, width: u16, height: u16, focused: bool) -> String {
        let buf = testgrid::draw_with(width, height, |frame| {
            let block = Rect::new(0, 0, width, height);
            overlay(app, block, focused, frame)
        });
        let mut s = String::new();
        for x in 0..width {
            if let Some(cell) = buf.cell((x, height - 1)) {
                s.push_str(cell.symbol());
            }
        }
        s
    }

    #[test]
    fn crumb_parts_relativizes_against_a_set_root() {
        let parts = crumb_parts(
            Path::new("/Users/xiii/vault/notes/note.md"),
            Path::new("/Users/xiii/vault"),
        );
        assert_eq!(parts, vec!["vault", "notes", "note.md"]);
    }

    #[test]
    fn crumb_parts_falls_back_to_the_absolute_path_outside_root() {
        let parts = crumb_parts(
            Path::new("/Users/xiii/other/note.md"),
            Path::new("/Users/xiii/vault"),
        );
        assert_eq!(parts, vec!["Users", "xiii", "other", "note.md"]);
    }

    #[test]
    fn crumb_parts_falls_back_to_the_absolute_path_when_root_is_unresolved() {
        let parts = crumb_parts(Path::new("/Users/xiii/vault/note.md"), Path::new(""));
        assert_eq!(parts, vec!["Users", "xiii", "vault", "note.md"]);
    }

    /// `/a/vault2` must never be treated as under root `/a/vault` —
    /// `Path::starts_with` compares whole components, unlike a bare string
    /// prefix check (the bug Go's own `buildCrumb` comment calls out).
    #[test]
    fn crumb_parts_does_not_mistake_a_sibling_with_a_shared_prefix_for_being_under_root() {
        let parts = crumb_parts(Path::new("/a/vault2/notes.md"), Path::new("/a/vault"));
        assert_eq!(parts, vec!["a", "vault2", "notes.md"]);
    }

    #[test]
    fn overlay_relativizes_against_app_root_end_to_end() {
        let mut app = app_for("hello", Some("/Users/xiii/vault/notes/note.md"));
        app.set_root(PathBuf::from("/Users/xiii/vault"));
        // parts = ["vault", "notes", "note.md"] instead of the full
        // ["Users", "xiii", "vault", "notes", "note.md"].
        let row = overlay_bottom_row(&app, 60, 3, true);
        assert!(row.contains("vault/notes › note.md"));
        assert!(!row.contains("Users"));
    }

    /// The crumb is root-relative for a document opened WITHOUT the
    /// Explorer pane ever being shown: the root comes from startup's
    /// `workspaceroot::resolve` → `App::set_root`, which runs
    /// unconditionally, so the Explorer's own state can't be a
    /// precondition for relativizing. The left column stays hidden
    /// throughout — it is never shown.
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

    /// The boundary the relativizing must NOT cross: a sibling directory
    /// sharing root's name as a string prefix is outside root, so its
    /// document falls back to the absolute path.
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

    /// The separator glyphs must be ONE display column each, or every width
    /// in this module (dash fill, `bc`, the truncation budget) is computed
    /// against a lie and the `──╯` drifts off the right edge (§1.5).
    #[test]
    fn the_separator_glyphs_are_single_column() {
        assert_eq!(text_width(SEP), 1);
        assert_eq!(text_width(LEAF_SEP), 3);
        assert_eq!(text_width(ELLIPSIS), 2);
    }

    /// A one-component path is all leaf: no separator, no stray padding.
    #[test]
    fn a_single_component_path_renders_just_the_leaf() {
        let theme = crate::theme::Theme::catppuccin_mocha(false);
        let segments = build_crumb(&["note.md".to_string()], 40, &theme);
        let text: String = segments.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "note.md");
    }

    #[test]
    fn renders_the_exact_row_at_a_known_width() {
        let app = app_for("hello", Some("/a/b/note.md"));
        // parts = ["a", "b", "note.md"]; crumb = "a/b › note.md" — the
        // directories run together on a bare `/`, the leaf is set off by
        // ` › `, and no part carries padding of its own: 1 + 1 + 1 + 3 + 7
        // = 13 columns. `overlay` adds the ONE plain space on each side.
        // At width 40, dash = 40 - 13 - 6 = 21.
        let row = overlay_bottom_row(&app, 40, 3, true);
        assert_eq!(row, format!("╰{} a/b › note.md ──╯", "─".repeat(21)));
    }

    /// Once the parts no longer fit, the dropped leading directories are
    /// replaced by `…/` — and the row must still end flush at the right
    /// edge, since the ellipsis is part of `bc` like any other span.
    #[test]
    fn a_too_long_path_is_truncated_with_an_ellipsis_prefix() {
        const W: u16 = 28;
        let app = app_for("hello", Some("/alpha/bravo/charlie/delta/note.md"));
        let row = overlay_bottom_row(&app, W, 3, true);
        // "…/delta › note.md" is 17 wide, so dash = 28 - 17 - 6 = 5.
        assert_eq!(row, format!("╰{} …/delta › note.md ──╯", "─".repeat(5)));
        assert_eq!(text_width(&row), W as usize, "the row must fill its width");
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
        let buf = testgrid::draw_with(40, 3, |_frame| {});
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
        let theme = crate::theme::Theme::catppuccin_mocha(false);
        for width in 30u16..80 {
            let block = Rect::new(0, 0, width, 3);
            let parts: Vec<String> = vec!["a".into(), "b".into(), "note.md".into()];
            let segments = build_crumb(&parts, width as usize, &theme);
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
