//! Renders the fuzzy file finder overlay that replaces the Explorer's own
//! content while `App::filesearch` is open: row 0 is the query row, reusing
//! `render::search::build_spans` (the search bar's own chokepoint) rather
//! than forking the prompt/caret/readout logic; the remaining rows are the
//! ranked result list — empty until a later work package's `Cmd`s populate
//! `recents`/`walk`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::filesearch::FileSearchState;

/// Pure function of `&App`, drawing into the same rect `explorer::draw`
/// would otherwise fill (`render::draw_left_pane`'s own branch). A no-op if
/// the finder isn't open or the rect has no rows at all.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let Some(state) = app.filesearch.as_ref() else {
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

    // The result rows themselves are empty in this work package — nothing
    // has populated `recents`/`walk` yet, so there is nothing further to
    // paint below the query row.
}

/// The query row's right-aligned readout: `matched/total` ordinarily, or
/// `scanning…` while a walk `Cmd` is in flight, with a `+truncated` suffix
/// when the walk hit its cap.
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
            readout_text(app.filesearch.as_ref().expect("open")),
            Some("scanning\u{2026}".to_string())
        );
    }

    #[test]
    fn readout_shows_matched_over_total_once_the_scan_reply_lands() {
        let mut app = app();
        app.root = PathBuf::from("/root");
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch.as_ref().expect("open").generation;

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
            readout_text(app.filesearch.as_ref().expect("open")),
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
            readout_text(app.filesearch.as_ref().expect("open")),
            Some("scanning\u{2026}".to_string())
        );

        let generation = app.filesearch.as_ref().expect("open").generation;
        if let Some(state) = app.filesearch.as_mut() {
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

        let state = app.filesearch.as_ref().expect("still open");
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
        assert!(app.filesearch.is_none());
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
}
