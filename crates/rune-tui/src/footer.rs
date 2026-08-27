use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::footer_hints::{default_hint_spans, truncated_default_hint_spans};
use crate::footer_modes::{disk_changed_spans, guard_spans, hint_row, merge_hint_spans};
use crate::guard::GuardPrompt;
use crate::pane::Pane;
use crate::width::display_width;
use rune_syntax::wrap::line_visual_col;

/// Which single visual state the footer's left side shows for this
/// render. Mutually exclusive by construction — never concatenated — and
/// checked by `mode()` in priority order, highest first: `Guard`, then the
/// focused Messages pane's own keys, a pending chord hint, the
/// degraded-store banner, the merge/disk-changed ambient hints, and
/// finally the default keymap hints.
enum Mode<'a> {
    /// Carries the whole `GuardPrompt`, not just its `GuardKind`:
    /// `guard_spans` needs `prompt.doc` to name WHICH document a
    /// `DirtyQuit` prompt is blocking on — a `GuardKind` alone can't say
    /// that.
    Guard(&'a GuardPrompt),
    Messages,
    ChordPending(String),
    Degraded(&'a str),
    DiskChanged,
    MergeHint(usize),
    DefaultHints,
}

fn mode(app: &App) -> Mode<'_> {
    if let Some(prompt) = &app.guard {
        return Mode::Guard(prompt);
    }
    if app.focus() == Pane::Messages {
        return Mode::Messages;
    }
    if let Some(hint) = chord_hint(app) {
        return Mode::ChordPending(hint);
    }
    if let Some(banner) = &app.db_banner {
        return Mode::Degraded(banner);
    }
    // Gated on the merge doc being the ACTIVE one, same as
    // `merge/keys.rs`'s own intercept, or switching tabs mid-merge would
    // keep showing the resolver's hints for a document not on screen.
    if let crate::merge::MergeState::Active { doc, .. } = app.merge
        && doc == app.active
    {
        return Mode::MergeHint(app.merge.unresolved_count());
    }
    // A `Pending` merge attempt's own invitation would be stale advice
    // about the very thing already in flight, so this only fires once
    // fully `Inactive`.
    if matches!(app.merge, crate::merge::MergeState::Inactive)
        && app
            .active_doc()
            .last_sync
            .is_some_and(rune_db::SyncKind::is_disk_divergent)
    {
        return Mode::DiskChanged;
    }
    Mode::DefaultHints
}

fn chord_hint(app: &App) -> Option<String> {
    if matches!(app.quit, crate::app::QuitNegotiation::ConfirmArmed(..)) {
        return Some(quit_hint(app).to_string());
    }
    if app
        .pending_save_confirm
        .is_some_and(|(cid, _)| cid == app.active)
    {
        let save_key = crate::global::label_for(crate::global::GlobalCommand::Save);
        return Some(format!("press {save_key} again to save anyway"));
    }
    None
}

fn quit_hint(app: &App) -> &'static str {
    if app.is_dirty() {
        "press again to quit \u{2014} unsaved changes will be lost"
    } else {
        "press again to quit"
    }
}

fn left_spans(app: &App) -> Vec<Span<'static>> {
    match mode(app) {
        Mode::Guard(prompt) => guard_spans(app, prompt),
        Mode::Messages => hint_row(app, [("\u{2318}C", "copy"), ("Esc", "close")]),
        Mode::ChordPending(text) => vec![Span::styled(text, app.theme.chrome.footer_key)],
        Mode::Degraded(msg) => vec![Span::styled(msg.to_string(), app.theme.chrome.footer_hint)],
        Mode::DiskChanged => disk_changed_spans(app),
        Mode::MergeHint(unresolved) => merge_hint_spans(app, unresolved),
        Mode::DefaultHints => default_hint_spans(app),
    }
}

pub fn footer_text(app: &App) -> String {
    left_spans(app).iter().map(|s| s.content.as_ref()).collect()
}

/// `BufferAhead` is deliberately excluded — that's the dirty flag's job.
/// Unlike the mutually-exclusive left-side `Mode`s, this rides the
/// always-rendered right side, so it survives Guard/Messages/Chord modes
/// shadowing the `DiskChanged` banner.
fn sync_marker(app: &App) -> &'static str {
    if app
        .active_doc()
        .last_sync
        .is_some_and(rune_db::SyncKind::is_disk_divergent)
    {
        "\u{21c4} "
    } else {
        ""
    }
}

/// Col is a line-relative terminal-cell column, not a wrap-row-relative
/// one: `line_visual_col` walks the cursor's own logical line from its own
/// start, so a wrapped line's second-and-later visual row never resets the
/// readout back to `Col 1`.
pub fn position_text(app: &App) -> String {
    let doc = app.active_doc();
    let offset = doc.cursors.primary().position.get();
    let bp = doc.buffer.offset_to_line_col(offset);
    let line_text = match (doc.buffer.line_start(bp.line), doc.buffer.line_end(bp.line)) {
        (Some(start), Some(end)) => doc.buffer.slice(start, end).unwrap_or(""),
        _ => "",
    };
    let col = line_visual_col(line_text, bp.col);
    format!("Ln {}, Col {}", bp.line + 1, col + 1)
}

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let bg = app.theme.chrome.footer;
    let marker = sync_marker(app);
    let right_text = position_text(app);
    let right_width = display_width(marker) + display_width(&right_text);
    let available = area.width as usize;
    // Truncation only applies to `DefaultHints` — the one mode that grows
    // with the focused pane's own hints; every other mode's content is
    // already short and renders exactly as `left_spans` produces it.
    let mut spans = match mode(app) {
        Mode::DefaultHints => truncated_default_hint_spans(app, available, right_width),
        _ => left_spans(app),
    };
    let left_width: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    if available > left_width + right_width {
        spans.push(Span::styled(
            " ".repeat(available - left_width - right_width),
            bg,
        ));
    }
    if !marker.is_empty() {
        spans.push(Span::styled(marker, app.theme.chrome.error));
    }
    spans.push(Span::styled(right_text, app.theme.chrome.footer_meta));
    frame.render_widget(Paragraph::new(Line::from(spans)).style(bg), area);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::keymap::{GLOBAL_BINDINGS, GlobalCommand, QuitKey};
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    #[test]
    fn merge_hint_is_suppressed_when_the_merge_document_is_not_active() {
        let mut app = app_with("hello");
        let merge_doc = app.active;
        let other = app.open_document(Buffer::new("scratch"));
        app.active = other;
        app.merge = crate::merge::MergeState::Active {
            doc: merge_doc,
            session: crate::merge::MergeSession {
                conflicts: Vec::new(),
                cur: 0,
                saved_display_name: None,
                theirs_obs: rune_db::ObsId::new(1).expect("nonzero"),
                install_pos: 0,
            },
        };

        let text = footer_text(&app);
        assert!(
            !text.contains('⚙') && !text.contains("O ours"),
            "merge hint leaked onto an inactive document's footer: {text:?}"
        );
    }

    #[test]
    fn default_mode_lists_every_global_binding_label() {
        let app = app_with("hello");
        assert_eq!(app.focus(), Pane::Editor);
        let text = footer_text(&app);
        for binding in GLOBAL_BINDINGS.iter().filter(|b| !b.secondary) {
            // `Merge` is conditional on divergence and pinned by the two
            // dedicated footer_hints tests; seeding divergence here would
            // flip the footer into `DiskChanged` mode instead.
            if matches!(binding.cmd, GlobalCommand::Merge) {
                continue;
            }
            assert!(
                text.contains(binding.help),
                "expected {:?} in default footer text {text:?}",
                binding.help
            );
        }
    }

    #[test]
    fn explorer_focus_shows_its_own_keys_and_omits_save() {
        let mut app = app_with("hello");
        // The left column starts hidden, and `set_focus_pane` falls back
        // to the Editor unless the target pane is actually painted.
        app.splits.left.show();
        app.set_focus_pane(Pane::Explorer, &mut crate::runtime::Effects::default());
        let text = footer_text(&app);
        assert!(text.contains("up dir"), "footer text: {text:?}");
        assert!(!text.contains("save"), "footer text: {text:?}");
    }

    #[test]
    fn tabs_focus_shows_its_own_keys() {
        let mut app = app_with("hello");
        app.splits.left.show();
        app.set_focus_pane(Pane::Tabs, &mut crate::runtime::Effects::default());
        let text = footer_text(&app);
        assert!(text.contains("close"), "footer text: {text:?}");
    }

    #[test]
    fn pending_quit_shows_the_quit_hint_over_the_degraded_banner() {
        let mut app = app_with("hello");
        app.db_banner = Some("recovery disabled: boom".to_string());
        app.quit = crate::app::QuitNegotiation::ConfirmArmed(
            QuitKey::CtrlC,
            crate::generation::Generation::ZERO,
        );
        assert_eq!(footer_text(&app), "press again to quit");
    }

    #[test]
    fn degraded_banner_outranks_the_default_hints() {
        let mut app = app_with("hello");
        app.db_banner = Some("recovery disabled: boom".to_string());
        crate::messages::info(&mut app, "some other message");
        assert_eq!(footer_text(&app), "recovery disabled: boom");
    }

    #[test]
    fn position_text_reports_one_indexed_line_and_col() {
        let app = app_with("hello");
        assert_eq!(position_text(&app), "Ln 1, Col 1");
    }

    // `footer_text` covers only the left side; this renders the full row,
    // including the ⇄ marker and position readout on the right.
    fn footer_row(app: &App, width: u16) -> String {
        let buf = crate::testgrid::draw_with(width, 1, |frame| {
            draw(app, Rect::new(0, 0, width, 1), frame)
        });
        (0..width)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()))
            .collect()
    }

    // Asserted on the `"⇄ Ln"` juxtaposition, not a bare `contains('⇄')`:
    // the `DiskChanged` left-side banner uses the same glyph.
    #[test]
    fn footer_right_side_shows_the_sync_marker_only_when_disk_diverged() {
        for last_sync in [
            None,
            Some(rune_db::SyncKind::Clean),
            Some(rune_db::SyncKind::BufferAhead),
        ] {
            let mut app = app_with("hello");
            app.active_doc_mut().last_sync = last_sync;
            let row = footer_row(&app, 80);
            assert!(
                !row.contains('\u{21c4}'),
                "expected no sync marker for {last_sync:?}: {row:?}"
            );
        }

        for last_sync in [rune_db::SyncKind::DiskAhead, rune_db::SyncKind::Diverged] {
            let mut app = app_with("hello");
            app.active_doc_mut().last_sync = Some(last_sync);
            let row = footer_row(&app, 80);
            assert!(
                row.contains("\u{21c4} Ln"),
                "expected the sync marker beside the position readout for {last_sync:?}: {row:?}"
            );
        }
    }

    #[test]
    fn footer_hint_row_shows_a_maximal_prefix_of_whole_hints_at_width_120() {
        let app = app_with("hello");
        assert_eq!(app.focus(), Pane::Editor);
        let row = footer_row(&app, 120);
        assert!(
            row.contains("^E messages"),
            "expected the table's tail hint to still fit at width 120: {row:?}"
        );
        assert!(
            !row.contains("trash"),
            "expected 'trash' — and no partial fragment of it — past the width-120 cutoff: {row:?}"
        );
    }

    #[test]
    fn footer_hint_row_degrades_to_fewer_whole_hints_at_a_narrower_width() {
        let app = app_with("hello");
        assert_eq!(app.focus(), Pane::Editor);
        let row = footer_row(&app, 30);
        assert!(row.contains("^S save"), "expected 'save' to fit: {row:?}");
        assert!(
            !row.contains("explorer"),
            "expected 'explorer' — and no partial fragment of it — dropped at width 30: {row:?}"
        );
    }

    #[test]
    fn footer_row_does_not_panic_at_degenerate_widths() {
        for width in [0u16, 1, 20] {
            let app = app_with("hello");
            let _ = footer_row(&app, width);
        }
    }

    #[test]
    fn save_confirm_hint_names_the_save_chord() {
        let mut app = app_with("hello");
        app.pending_save_confirm = Some((app.active, crate::generation::Generation::ZERO));
        assert_eq!(footer_text(&app), "press ^S again to save anyway");
    }

    #[test]
    fn save_confirm_hint_is_scoped_to_the_document_it_was_armed_on() {
        let mut app = app_with("hello");
        let doc_a = app.active;
        let doc_b = app.open_document(Buffer::new("world"));
        app.pending_save_confirm = Some((doc_a, crate::generation::Generation::ZERO));

        assert_eq!(app.active, doc_a);
        assert!(
            footer_text(&app).contains("save anyway"),
            "doc A is active: its own pending confirm hint must show"
        );

        app.active = doc_b;
        assert!(
            !footer_text(&app).contains("save anyway"),
            "doc B is active: doc A's stale pending confirm hint must not show"
        );

        app.active = doc_a;
        assert!(
            footer_text(&app).contains("save anyway"),
            "switching back to doc A must show its hint again"
        );
    }
}
