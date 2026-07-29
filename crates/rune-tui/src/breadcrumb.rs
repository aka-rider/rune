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
//!
//! Every width in this module — the `bc` total, `build_crumb`'s per-part
//! accounting, and `put`'s column advance — goes through the crate's ONE
//! chrome-width chokepoint (`crate::width::display_width`, backed by
//! `rune_syntax::wrap::grapheme_width`), one grapheme CLUSTER per cell
//! (§1.5), so the dash fill can never be sized in one unit and drawn in
//! another: a CJK/emoji/NFD-accented path component makes the difference
//! visible immediately if the two ever drift apart.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use std::path::Component;
use unicode_segmentation::UnicodeSegmentation;

use crate::app::App;
use crate::width::display_width;
use rune_syntax::wrap::grapheme_width;

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
        .map(|s| display_width(s.content.as_ref()))
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

    put(buf, &mut x, y, "╰", border_style);
    for _ in 0..dash {
        put(buf, &mut x, y, "─", border_style);
    }
    put(buf, &mut x, y, " ", plain);
    for span in &segments {
        for cluster in span.content.graphemes(true) {
            put(buf, &mut x, y, cluster, span.style);
        }
    }
    put(buf, &mut x, y, " ", plain);
    for cluster in "──╯".graphemes(true) {
        put(buf, &mut x, y, cluster, border_style);
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
        let part_width = display_width(part);
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
                part_width + display_width(sep),
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

/// Writes one whole GRAPHEME CLUSTER at `(*x, y)` and advances `*x` by that
/// cluster's DISPLAY width — the identical idiom `render::blit` uses
/// (`x.saturating_add(cell.width.max(1) as u16)`), and the reason this
/// module can splice into a border row at all. One cluster per `Cell`
/// (`cell_mut().set_symbol`, not `set_char`) rather than one `char` per
/// `Cell`: advancing/writing per-`char` while `overlay` sizes its dash fill
/// per grapheme cluster would desync the two the moment a path component
/// holds an NFD accent or a ZWJ emoji sequence — the accent/joiner runes
/// would each claim their own (wrong) cell instead of riding along in the
/// base character's cell, and the `──╯` would land short of the right
/// edge, leaving stale border cells behind it.
///
/// A cluster whose width is more than 1 (a CJK ideograph, a wide emoji)
/// claims MORE than one `Cell` on screen but this function only ever
/// writes its symbol into the FIRST one: like `render::blit`, it resets
/// every continuation cell the cluster covers, or whatever the buffer held
/// there before (a border dash, a previous glyph's leftover) stays visible
/// beside the new symbol — a real on-screen artifact, not just a test
/// nicety, since two glyphs would then appear to occupy the same visual
/// span. Out-of-buffer writes are dropped by `cell_mut` returning `None`.
fn put(buf: &mut ratatui::buffer::Buffer, x: &mut u16, y: u16, cluster: &str, style: Style) {
    if let Some(cell) = buf.cell_mut((*x, y)) {
        cell.set_symbol(cluster);
        cell.set_style(style);
    }
    let width = grapheme_width(cluster).max(1) as u16;
    for dx in 1..width {
        if let Some(cont) = buf.cell_mut((x.saturating_add(dx), y)) {
            cont.reset();
        }
    }
    *x = x.saturating_add(width);
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
        // Mirrors ratatui's OWN diffing (`Buffer::diff`, `set_stringn`):
        // a wide glyph's continuation cell is a reset (blank) `Cell` that
        // real terminal output never reaches, because the renderer
        // recomputes the PRECEDING cell's own display width and skips that
        // many columns unconditionally — it never consults the
        // continuation cell's content. Reading every raw cell symbol
        // (including that skipped one) would double-count a column no
        // terminal ever prints, so this walk skips ahead by each symbol's
        // own width exactly like the real render path does.
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
    /// precondition for relativizing. `left_visible` stays `false`
    /// throughout — the pane is never toggled on.
    #[test]
    fn the_crumb_is_root_relative_without_the_explorer_ever_being_shown() {
        let mut app = app_for("hello", Some("/Users/xiii/vault/notes/note.md"));
        app.set_root(PathBuf::from("/Users/xiii/vault"));
        assert!(!app.left_visible, "the Explorer pane must not be shown");

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

    /// An independent width oracle (plan [rune-tui C 14]): computed
    /// straight from `unicode_width`/`unicode_segmentation`, never by
    /// calling this module's own `display_width`/`grapheme_width` — so a
    /// regression in the production chokepoint can't pass a test that
    /// merely re-invokes it. The old per-`char` `text_width` this module
    /// used to have made its own width test "agree by construction" with
    /// exactly the bug it existed to catch.
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

    /// The separator glyphs must be ONE display column each, or every width
    /// in this module (dash fill, `bc`, the truncation budget) is computed
    /// against a lie and the `──╯` drifts off the right edge (§1.5).
    #[test]
    fn the_separator_glyphs_are_single_column() {
        assert_eq!(oracle_cell_width(SEP), 1);
        assert_eq!(oracle_cell_width(LEAF_SEP), 3);
        assert_eq!(oracle_cell_width(ELLIPSIS), 2);
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
        assert_eq!(
            oracle_cell_width(&row),
            W as usize,
            "the row must fill its width"
        );
    }

    /// An NFD-decomposed filename (`café.md` as `e` + combining acute,
    /// macOS's own on-disk normalization) must render its accent riding
    /// along in the base letter's cell, not claiming a stray cell of its
    /// own — the exact class [rune-tui C 2] found broken. Measured against
    /// the independent oracle, never `display_width` itself.
    #[test]
    fn an_nfd_accent_rides_along_in_its_base_letters_cell() {
        const W: u16 = 40;
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

    /// A ZWJ-joined emoji family in a path component is one grapheme
    /// cluster, one cell — never torn into several cells the way a bare
    /// `chars()` walk would.
    #[test]
    fn a_zwj_emoji_family_component_occupies_one_cell_per_cluster() {
        const W: u16 = 40;
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
                .map(|s| display_width(s.content.as_ref()))
                .sum();
            if bc + 7 > block.width as usize {
                continue;
            }
            let dash = block.width as usize - bc - 6;
            assert_eq!(1 + dash + (bc + 2) + 3, block.width as usize);
        }
    }
}
