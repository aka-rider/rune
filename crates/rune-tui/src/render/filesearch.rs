//! Renders the fuzzy file finder overlay that replaces the Explorer's own
//! content while `App::filesearch` is open: row 0 is the query row, reusing
//! `render::search::build_spans` (the search bar's own chokepoint) rather
//! than forking the prompt/caret/readout logic; the remaining rows are the
//! ranked result list, styled purely from the precomputed `ResultRow::
//! indices` `update`'s own `recompute` chokepoint already computed — this
//! module never scores or ranks anything itself.

use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;

use crate::app::App;
use crate::filesearch::{FileSearchState, ResultRow, candidate_at};
use crate::pane::Pane;
use crate::theme::Theme;
use crate::width::display_width;

/// Pure function of `&App`, drawing into the same rect `explorer::draw`
/// would otherwise fill (`render::draw_left_pane`'s own branch). A no-op if
/// the finder isn't open or the rect has no rows at all.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let Some(state) = app.filesearch() else {
        return;
    };
    if area.height == 0 {
        return;
    }

    let bar_area = Rect::new(area.x, area.y, area.width, 1);
    let readout = readout_text(state);
    let spans = crate::render::search::build_spans(
        &state.query,
        readout.as_deref(),
        true,
        area.width as usize,
        &app.theme,
    );
    frame.render_widget(Paragraph::new(Line::from(spans)), bar_area);

    let rows_height = area.height.saturating_sub(1);
    if rows_height == 0 {
        return;
    }
    let rows_area = Rect::new(area.x, area.y + 1, area.width, rows_height);
    let lines = result_lines(app, state, rows_height as usize, area.width as usize);
    frame.render_widget(Paragraph::new(lines), rows_area);
}

/// The result rows themselves: an explicit "no matches" feedback row for a
/// non-empty query nothing matched (never a blank pane — house rule: silent
/// input swallowing is architecturally unsound), otherwise the nav-windowed
/// slice of `state.results`.
fn result_lines(
    app: &App,
    state: &FileSearchState,
    rows: usize,
    width: usize,
) -> Vec<Line<'static>> {
    if state.results.is_empty() && !state.query.trim().is_empty() {
        return vec![Line::from(Span::styled(
            "no matches",
            Style::new().fg(app.theme.chrome.subtle),
        ))];
    }

    let focused = app.focus() == Pane::Explorer;
    let window = state.nav.window(state.results.len(), rows);
    let start = window.start;
    let visible = state.results.get(window).unwrap_or(&[]);
    visible
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let selected = start + i == state.nav.cursor;
            result_line(app, state, row, selected, focused, width)
        })
        .collect()
}

/// One result row: a `›`-prefixed cursor row (full-row background rect,
/// only while the Explorer pane is actually focused) or a plain two-space
/// gutter; the candidate's own display string styled by [`display_spans`].
fn result_line(
    app: &App,
    state: &FileSearchState,
    row: &ResultRow,
    selected: bool,
    focused: bool,
    width: usize,
) -> Line<'static> {
    let row_bg = (selected && focused).then_some(app.theme.chrome.selection_bg);
    let prefix = if selected { "\u{203a} " } else { "  " };
    let mut spans = vec![Span::styled(
        prefix.to_string(),
        with_bg(app.theme.chrome.file_normal, row_bg),
    )];

    if let Some(candidate) = candidate_at(state, row.candidate_idx) {
        let avail = width.saturating_sub(display_width(prefix));
        spans.extend(display_spans(
            &candidate.display,
            &row.indices,
            &app.theme,
            avail,
            row_bg,
        ));
    }

    let content_w: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    if width > content_w {
        spans.push(Span::styled(
            " ".repeat(width - content_w),
            with_bg(app.theme.chrome.file_normal, row_bg),
        ));
    }
    Line::from(spans)
}

/// Styles one candidate's `display` string: the directory portion (up to
/// and including the last `/`) dimmed, the filename portion in the `text`
/// hue — no blue, reserved for a directory row the finder's results (files
/// only) never show — and every grapheme `indices` names rendered bold on
/// top of whichever base colour it falls under. Left-truncates to `avail_w`
/// cells (leading `…`, tail kept, the `truncate_root`/`truncate_tail_to_
/// width` idiom) by taking a SUFFIX of the already-styled grapheme list,
/// so a truncated row's surviving bold/dim styling never has to be
/// re-derived from a byte offset shifted by the cut.
fn display_spans(
    display: &str,
    indices: &[u32],
    theme: &Theme,
    avail_w: usize,
    row_bg: Option<Color>,
) -> Vec<Span<'static>> {
    let dir_end = display.rfind('/').map(|i| i + 1).unwrap_or(0);
    let graphemes: Vec<(usize, &str)> = display.grapheme_indices(true).collect();
    let matched_graphemes = grapheme_match_mask(display, &graphemes, indices);
    let total_w = display_width(display);
    let (start, truncated) = fit_suffix(&graphemes, total_w, avail_w);

    let dim_style = with_bg(Style::new().fg(theme.chrome.subtle), row_bg);
    let file_style = with_bg(theme.chrome.file_normal, row_bg);

    let mut spans = Vec::with_capacity(graphemes.len() - start + 1);
    if truncated {
        spans.push(Span::styled("\u{2026}".to_string(), dim_style));
    }
    for (grapheme_idx, (byte_off, g)) in graphemes.iter().enumerate().skip(start) {
        let base = if *byte_off < dir_end {
            dim_style
        } else {
            file_style
        };
        let style = if matched_graphemes
            .get(grapheme_idx)
            .copied()
            .unwrap_or(false)
        {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        };
        spans.push(Span::styled((*g).to_string(), style));
    }
    spans
}

/// Maps nucleo's `indices` onto a per-grapheme bold mask, mirroring
/// `Utf32Str::new`'s own branch choice (nucleo-matcher 0.3.1, default
/// features): when every grapheme's leading codepoint is ASCII, nucleo
/// matches against the display string's raw UTF-8 BYTES — a whole-ASCII
/// name always takes this path, and so does an NFD name whose combining
/// marks all trail an ASCII base (routine on macOS/APFS: "é" as `e` +
/// U+0301) since the base codepoint alone is what survives nucleo's own
/// per-grapheme reduction — so `indices` there are byte offsets, matched
/// against each grapheme's own starting byte. Otherwise (a grapheme led by
/// a non-ASCII codepoint, e.g. precomposed "é" or CJK) nucleo matches one
/// codepoint per grapheme and `indices` are grapheme POSITIONS directly.
fn grapheme_match_mask(display: &str, graphemes: &[(usize, &str)], indices: &[u32]) -> Vec<bool> {
    let matched: HashSet<usize> = indices.iter().map(|&i| i as usize).collect();
    let ascii_reduced = display.is_ascii()
        || graphemes
            .iter()
            .all(|(_, g)| g.chars().next().is_some_and(|c| c.is_ascii()));
    if ascii_reduced {
        graphemes
            .iter()
            .map(|(byte_off, _)| matched.contains(byte_off))
            .collect()
    } else {
        (0..graphemes.len()).map(|i| matched.contains(&i)).collect()
    }
}

/// The longest SUFFIX of `graphemes` (grapheme-boundary cuts only) that
/// fits `avail_w` cells alongside a leading `…` when `total_w` overruns it
/// — the styling-aware sibling of `width::truncate_tail_to_width`, which
/// this can't reuse directly since it needs to keep each surviving
/// grapheme's own index (for the bold-matched lookup), not just the
/// resulting string.
fn fit_suffix(graphemes: &[(usize, &str)], total_w: usize, avail_w: usize) -> (usize, bool) {
    if total_w <= avail_w {
        return (0, false);
    }
    let ellipsis_w = display_width("\u{2026}");
    let budget = avail_w.saturating_sub(ellipsis_w);
    let mut used = 0usize;
    let mut start = graphemes.len();
    for (i, (_, g)) in graphemes.iter().enumerate().rev() {
        let w = display_width(g);
        if used + w > budget {
            break;
        }
        used += w;
        start = i;
    }
    (start, true)
}

fn with_bg(style: Style, bg: Option<Color>) -> Style {
    match bg {
        Some(color) => style.bg(color),
        None => style,
    }
}

/// The query row's right-aligned readout: `matched/total` ordinarily, or
/// `scanning…` while a walk `Cmd` is in flight, with a `+truncated` suffix
/// when the walk hit its cap. `matched` is `state.results.len()` — the
/// count actually shown, post the finder's own result cap — against
/// `total`, the full candidate pool, so a cap (either the result cap on a
/// broad match, or a walk truncation) stays visible as `matched < total`
/// without a second counter to keep in sync.
fn readout_text(state: &FileSearchState) -> Option<String> {
    if state.walk_pending {
        return Some("scanning\u{2026}".to_string());
    }
    let matched = state.results.len();
    let total = state.recents.len() + state.walk.len();
    let truncated = if state.walk_truncated {
        "+truncated"
    } else {
        ""
    };
    Some(format!("{matched}/{total}{truncated}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::filesearch::{Candidate, walk};
    use crate::runtime::CmdKind;
    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, Vfs};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.frame_width = 120;
        app.frame_height = 34;
        app
    }

    #[test]
    fn readout_shows_scanning_while_the_walk_is_pending() {
        let mut app = app();
        app.root = PathBuf::from("/root");
        let mut effects = crate::runtime::Effects::default();

        crate::filesearch::open(&mut app, &mut effects);

        assert_eq!(
            readout_text(app.filesearch().expect("open")),
            Some("scanning\u{2026}".to_string())
        );
    }

    #[test]
    fn readout_shows_matched_over_total_once_the_scan_reply_lands() {
        let mut app = app();
        app.root = PathBuf::from("/root");
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;

        crate::filesearch::handle_scanned(
            &mut app,
            generation,
            Ok(walk::ScanResult {
                files: Vec::new(),
                truncated: false,
            }),
            &mut effects,
        );

        assert_eq!(
            readout_text(app.filesearch().expect("open")),
            Some("0/0".to_string())
        );
    }

    /// The plan's own WP3.S4 acceptance case, driven end to end: `open`
    /// pushes the scan `Cmd` — inspected, never executed inline — and only
    /// once its reply is hand-delivered does the list settle into
    /// recents-then-walk order with the deduped path counted once, and the
    /// readout leave `scanning…` for a real `matched/total`.
    #[test]
    fn hand_delivered_scan_reply_lists_recents_then_walk_and_dedups_by_path() {
        let vfs = Mem::new();
        vfs.save_atomic(Path::new("/root/a.md"), b"a")
            .expect("seed a.md");
        vfs.save_atomic(Path::new("/root/b.md"), b"b")
            .expect("seed b.md");
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(vfs), None);
        app.frame_width = 120;
        app.frame_height = 34;
        app.root = PathBuf::from("/root");
        let mut effects = crate::runtime::Effects::default();

        crate::filesearch::open(&mut app, &mut effects);
        assert!(
            effects.cmds.iter().any(|c| c.kind() == CmdKind::ReadDir),
            "open pushes the scan Cmd rather than running it inline"
        );
        assert_eq!(
            readout_text(app.filesearch().expect("open")),
            Some("scanning\u{2026}".to_string())
        );

        let generation = app.filesearch().expect("open").generation;
        if let Some(state) = app.filesearch_mut() {
            state.recents.push(Candidate {
                path: PathBuf::from("/root/a.md"),
                display: "a.md".to_string(),
                in_tree: true,
                mru_rank: Some(0),
            });
        }

        crate::filesearch::handle_scanned(
            &mut app,
            generation,
            Ok(walk::ScanResult {
                files: vec![PathBuf::from("/root/a.md"), PathBuf::from("/root/b.md")],
                truncated: false,
            }),
            &mut effects,
        );

        let state = app.filesearch().expect("still open");
        assert_eq!(
            state
                .walk
                .iter()
                .map(|c| c.path.clone())
                .collect::<Vec<_>>(),
            vec![PathBuf::from("/root/b.md")],
            "a.md is already covered by a recent, so it's dropped from walk"
        );
        assert_eq!(state.results.len(), 2);
        assert_eq!(
            state
                .results
                .first()
                .and_then(|r| crate::filesearch::candidate_at(state, r.candidate_idx))
                .map(|c| c.path.clone()),
            Some(PathBuf::from("/root/a.md")),
            "recents occupy the low flat indices, ahead of walk"
        );
        assert_eq!(
            state
                .results
                .get(1)
                .and_then(|r| crate::filesearch::candidate_at(state, r.candidate_idx))
                .map(|c| c.path.clone()),
            Some(PathBuf::from("/root/b.md"))
        );
        assert_eq!(readout_text(state), Some("2/2".to_string()));
    }

    #[test]
    fn draw_is_a_no_op_when_the_finder_is_closed() {
        let app = app();
        assert!(app.filesearch().is_none());
        let buf = crate::testgrid::draw_with(20, 5, |frame| {
            draw(&app, Rect::new(0, 0, 20, 5), frame);
        });
        for y in 0..5 {
            for x in 0..20 {
                let cell = buf.cell((x, y)).expect("cell in bounds");
                assert_eq!(cell.symbol(), " ");
            }
        }
    }

    fn candidate(path: &str, display: &str) -> Candidate {
        Candidate {
            path: PathBuf::from(path),
            display: display.to_string(),
            in_tree: true,
            mru_rank: None,
        }
    }

    fn seeded_app(files: &[(&str, &str)]) -> App {
        let mem = Mem::new();
        for (path, content) in files {
            mem.save_atomic(Path::new(path), content.as_bytes())
                .expect("seed file");
        }
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(mem), None);
        app.frame_width = 120;
        app.frame_height = 34;
        app.root = PathBuf::from("/root");
        app
    }

    /// WP4.S4: a query with zero matches renders an explicit "no matches"
    /// feedback row rather than a blank pane.
    #[test]
    fn a_query_with_no_matches_renders_an_explicit_empty_state_row() {
        let mut app = seeded_app(&[("/root/a.md", "a")]);
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;
        crate::filesearch::handle_recents_loaded(
            &mut app,
            generation,
            Ok(vec![candidate("/root/a.md", "a.md")]),
            &mut effects,
        );
        if let Some(state) = app.filesearch_mut() {
            state.query = "zzzzzznomatch".to_string();
        }
        crate::filesearch::recompute(&mut app, &mut effects);
        let state = app.filesearch().expect("still open");
        assert!(state.results.is_empty(), "test setup: nothing matches");

        let lines = result_lines(&app, state, 5, 40);
        let text: String = lines
            .first()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default();
        assert_eq!(text, "no matches");
    }

    /// WP4.S4: two visible rows matching the same query have disjoint,
    /// per-row-correct bold spans — pins the `indices.clear()` requirement
    /// (`rank::rank`'s own doc): a callee that forgot to clear would leak
    /// the first row's matched positions into the second row's bold set.
    #[test]
    fn multi_row_highlight_indices_are_disjoint_and_per_row_correct() {
        let mut app = seeded_app(&[("/root/note-a.md", "a"), ("/root/note-b.md", "b")]);
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;
        crate::filesearch::handle_recents_loaded(
            &mut app,
            generation,
            Ok(vec![
                candidate("/root/note-a.md", "note-a.md"),
                candidate("/root/note-b.md", "note-b.md"),
            ]),
            &mut effects,
        );
        if let Some(state) = app.filesearch_mut() {
            state.query = "note".to_string();
        }
        crate::filesearch::recompute(&mut app, &mut effects);
        let state = app.filesearch().expect("still open");
        assert_eq!(state.results.len(), 2, "both candidates match \"note\"");

        for row in &state.results {
            let candidate = candidate_at(state, row.candidate_idx).expect("row names a candidate");
            assert_eq!(
                row.indices,
                vec![0, 1, 2, 3],
                "each row's own indices must name ITS OWN \"note\" prefix, \
                 not a leaked copy from the other row: {}",
                candidate.display
            );
        }
    }

    /// Finding 4: nucleo's `indices` are CHAR positions into the display
    /// string, not grapheme positions — an NFD-decomposed filename (routine
    /// on macOS/APFS) makes the two diverge. A query matching the ASCII
    /// tail of an NFD name must bold exactly those trailing graphemes, not
    /// shift onto the wrong character or vanish.
    #[test]
    fn nfd_decomposed_filename_bolds_the_matching_ascii_tail_graphemes() {
        let nfd_name = "cafe\u{0301}.md"; // "café.md", "e" + combining acute
        let mut app = seeded_app(&[]);
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;
        crate::filesearch::handle_recents_loaded(
            &mut app,
            generation,
            Ok(vec![candidate("/root/cafe.md", nfd_name)]),
            &mut effects,
        );
        if let Some(state) = app.filesearch_mut() {
            state.query = "md".to_string();
        }
        crate::filesearch::recompute(&mut app, &mut effects);
        let state = app.filesearch().expect("still open");
        let row = state
            .results
            .first()
            .expect("query \"md\" matches the NFD filename's ascii tail");

        let spans = display_spans(nfd_name, &row.indices, &app.theme, 80, None);
        let bold_graphemes: Vec<String> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(bold_graphemes, vec!["m".to_string(), "d".to_string()]);

        let non_bold_graphemes: Vec<String> = spans
            .iter()
            .filter(|s| !s.style.add_modifier.contains(Modifier::BOLD))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(
            non_bold_graphemes,
            vec![
                "c".to_string(),
                "a".to_string(),
                "f".to_string(),
                "e\u{0301}".to_string(),
                ".".to_string(),
            ],
            "the NFD e + combining-acute grapheme stays intact as one unmatched span"
        );
    }
}
