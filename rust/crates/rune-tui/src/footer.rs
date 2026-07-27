//! The footer row: a display-mode priority renderer, pure function of
//! `&App` (plan WP2.S6, port of Go's `footer_view.go:displayMode` priority
//! table — declaration order there IS priority order, highest first). File
//! renamed from `status.rs`: WP1's per-doc file-name/dirty-dot display
//! moves to WP6's `title.rs`; this module owns only the chrome-wide message
//! row + the always-visible cursor position.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, StatusSource};
use crate::banner::Modal;
use crate::keymap::GLOBAL_BINDINGS;
use crate::styles;

/// Which single visual state the footer's left side shows for this render
/// — priority order highest-first (plan WP2.S6, WP3.S3): a modal (the
/// banner) always wins over a save error, which wins over a pending chord
/// hint, which wins over the degraded-store banner, which wins over a plain
/// status message, which falls back to the default keymap hints. Mutually
/// exclusive by construction — never concatenated, unlike the pre-WP2
/// `status.rs::status_text`.
enum Mode<'a> {
    Modal,
    /// The close-confirmation prompt (plan WP5.S3) — outranks everything
    /// below it exactly like `Modal` does, since it's the SAME `App.modal`
    /// slot, just the other variant.
    Guard,
    SaveError(&'a str),
    ChordPending(String),
    Degraded(&'a str),
    Status(&'a str),
    DefaultHints,
}

fn mode(app: &App) -> Mode<'_> {
    match &app.modal {
        Some(Modal::Error(_)) => return Mode::Modal,
        Some(Modal::Guard(_)) => return Mode::Guard,
        None => {}
    }
    if app.status_source == StatusSource::SaveError
        && let Some(msg) = &app.status_message
    {
        return Mode::SaveError(msg);
    }
    if let Some(hint) = chord_hint(app) {
        return Mode::ChordPending(hint);
    }
    if let Some(banner) = &app.db_banner {
        return Mode::Degraded(banner);
    }
    if let Some(msg) = &app.status_message {
        return Mode::Status(msg);
    }
    Mode::DefaultHints
}

/// `pending_quit`'s hint has no other home (the pre-WP2 `quit_hint`, ported
/// unchanged below); `pending_save_confirm`'s hint text is already sitting
/// in `app.status_message` — `trigger_save` wrote it there the same tick it
/// armed the confirm gate — so this just surfaces that rather than
/// duplicating the string. The `cid == app.active` check is load-bearing:
/// `pending_save_confirm` is doc-tagged (armed for the document that
/// attempted the save), so switching tabs away from that document must not
/// leave its stale hint showing over whatever document is active now.
fn chord_hint(app: &App) -> Option<String> {
    if app.pending_quit.is_some() {
        return Some(quit_hint(app).to_string());
    }
    if app
        .pending_save_confirm
        .is_some_and(|(cid, _)| cid == app.active)
    {
        return app
            .status_message
            .clone()
            .or_else(|| Some("press \u{2318}S again to save anyway".to_string()));
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

/// The left side's styled spans for the current `Mode` — the pure content
/// `draw` renders and tests assert on via `footer_text` below.
fn left_spans(app: &App) -> Vec<Span<'static>> {
    match mode(app) {
        Mode::Modal => vec![
            Span::styled("[C]opy", styles::footer_key()),
            Span::styled("  ", styles::footer_hint()),
            Span::styled("[Esc] discard", styles::footer_hint()),
        ],
        Mode::Guard => vec![
            Span::styled("[S]ave", styles::footer_key()),
            Span::styled("  ", styles::footer_hint()),
            Span::styled("[D]iscard", styles::footer_key()),
            Span::styled("  ", styles::footer_hint()),
            Span::styled("[Esc] Cancel", styles::footer_hint()),
        ],
        Mode::SaveError(msg) => vec![Span::styled(msg.to_string(), styles::error())],
        Mode::ChordPending(text) => vec![Span::styled(text, styles::footer_key())],
        Mode::Degraded(msg) => vec![Span::styled(msg.to_string(), styles::footer_hint())],
        Mode::Status(msg) => vec![Span::styled(msg.to_string(), styles::footer_hint())],
        Mode::DefaultHints => default_hint_spans(),
    }
}

/// Default-mode hints (plan WP2.S6/S7): one `<key> label` pair per
/// `GLOBAL_BINDINGS` entry, in table order — the same table WP7's Help doc
/// iterates, so a chord's footer hint and its Help-doc line can never
/// drift apart. The two quit chords each render their own "quit" entry
/// (iterating the table literally, not de-duplicating by label).
fn default_hint_spans() -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, binding) in GLOBAL_BINDINGS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", styles::footer_hint()));
        }
        spans.push(Span::styled(binding.key.label(), styles::footer_key()));
        spans.push(Span::styled(" ", styles::footer_hint()));
        spans.push(Span::styled(binding.help, styles::footer_hint()));
    }
    spans
}

/// The footer's left-side text content, with no styling — for tests that
/// only need to assert on WHAT shows, not how (mirrors the pre-WP2
/// `status_text`'s role).
pub fn footer_text(app: &App) -> String {
    left_spans(app).iter().map(|s| s.content.as_ref()).collect()
}

/// `Ln <line>, Col <col>` from the active document's primary cursor (plan
/// Assumption A3, port of Go `footer_view.go:176`'s `m.line+1, m.col+1`) —
/// always shown, regardless of the left side's `Mode`. Col counts RUNES
/// within the line (§1.5), via `Buffer::display_position`.
pub fn position_text(app: &App) -> String {
    let doc = app.active_doc();
    let offset = doc.cursors.primary().position;
    let (line, col) = doc.buffer.display_position(offset);
    format!("Ln {line}, Col {col}")
}

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let bg = styles::footer();
    let mut spans = left_spans(app);
    let left_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let right_text = position_text(app);
    let right_width = right_text.chars().count();
    let available = area.width as usize;
    if available > left_width + right_width {
        spans.push(Span::styled(
            " ".repeat(available - left_width - right_width),
            bg,
        ));
    }
    spans.push(Span::styled(right_text, styles::footer_meta()));
    frame.render_widget(Paragraph::new(Line::from(spans)).style(bg), area);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::keymap::QuitKey;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    #[test]
    fn default_mode_lists_every_global_binding_label() {
        let app = app_with("hello");
        let text = footer_text(&app);
        for binding in GLOBAL_BINDINGS {
            assert!(
                text.contains(binding.help),
                "expected {:?} in default footer text {text:?}",
                binding.help
            );
        }
    }

    #[test]
    fn save_error_outranks_everything_else() {
        let mut app = app_with("hello");
        app.db_banner = Some("recovery disabled: boom".to_string());
        app.status_message = Some("status message".to_string());
        app.set_status("save failed: disk full", StatusSource::SaveError);
        assert_eq!(footer_text(&app), "save failed: disk full");
    }

    #[test]
    fn pending_quit_shows_the_quit_hint_over_the_degraded_banner() {
        let mut app = app_with("hello");
        app.db_banner = Some("recovery disabled: boom".to_string());
        app.pending_quit = Some((QuitKey::CtrlC, 0));
        assert_eq!(footer_text(&app), "press again to quit");
    }

    #[test]
    fn degraded_banner_outranks_a_plain_status_message() {
        let mut app = app_with("hello");
        app.db_banner = Some("recovery disabled: boom".to_string());
        app.set_status("some other message", StatusSource::Other);
        assert_eq!(footer_text(&app), "recovery disabled: boom");
    }

    #[test]
    fn position_text_reports_one_indexed_line_and_col() {
        let app = app_with("hello");
        assert_eq!(position_text(&app), "Ln 1, Col 1");
    }

    /// `pending_save_confirm` is doc-tagged (plan WP1 decision 3): a chord
    /// armed on doc A must not leak its hint onto doc B's footer after a
    /// tab switch, and must reappear once doc A is active again.
    #[test]
    fn save_confirm_hint_is_scoped_to_the_document_it_was_armed_on() {
        let mut app = app_with("hello");
        let doc_a = app.active;
        let doc_b = app.open_document(Buffer::new("world"));
        app.pending_save_confirm = Some((doc_a, 0));

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
