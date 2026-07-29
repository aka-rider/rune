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
use crate::banner;
use crate::banner::{GuardKind, Modal};
use crate::explorer::EXPLORER_BINDINGS;
use crate::global::{self, LEADER_BINDINGS};
use crate::keymap::{GLOBAL_BINDINGS, GlobalCommand};
use crate::opentabs::TABS_BINDINGS;
use crate::pane::Pane;

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
    Guard(&'a GuardKind),
    SaveError(&'a str),
    ChordPending(String),
    Degraded(&'a str),
    Status(&'a str),
    DefaultHints,
}

fn mode(app: &App) -> Mode<'_> {
    match &app.modal {
        Some(Modal::Error(_)) => return Mode::Modal,
        Some(Modal::Guard(prompt)) => return Mode::Guard(&prompt.kind),
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
            Span::styled("[C]opy", app.theme.chrome.footer_key),
            Span::styled("  ", app.theme.chrome.footer_hint),
            Span::styled("[Esc] discard", app.theme.chrome.footer_hint),
        ],
        Mode::Guard(kind) => guard_spans(app, kind),
        Mode::SaveError(msg) => vec![Span::styled(msg.to_string(), app.theme.chrome.error)],
        Mode::ChordPending(text) => vec![Span::styled(text, app.theme.chrome.footer_key)],
        Mode::Degraded(msg) => vec![Span::styled(msg.to_string(), app.theme.chrome.footer_hint)],
        Mode::Status(msg) => vec![Span::styled(msg.to_string(), app.theme.chrome.footer_hint)],
        Mode::DefaultHints => default_hint_spans(app),
    }
}

/// The dirty-close Guard's `[S]ave [D]iscard [Esc] Cancel` hint (plan
/// WP5.S3), built from `banner::DIRTY_CLOSE_OPTIONS`/`DIRTY_CLOSE_CANCEL_
/// LABEL` — the SAME consts `banner::handle_guard_key` matches its `s`/`d`
/// keys against, so this render can never drift from what those keys
/// actually do (review fix).
fn guard_spans(app: &App, kind: &GuardKind) -> Vec<Span<'static>> {
    let mut spans = Vec::new();

    // A rename collision names its target: "replace <what>?" is a question
    // the user can answer; a bare `[R]eplace` is not.
    let options: &[banner::GuardOption] = match kind {
        GuardKind::DirtyClose => banner::DIRTY_CLOSE_OPTIONS,
        GuardKind::RenameCollision { target } => {
            spans.push(Span::styled(
                format!("{target} already exists  "),
                app.theme.chrome.footer_hint,
            ));
            // §1.4.10: without a durable store there is nowhere to preserve
            // the replaced file's bytes, so the option is not offered at
            // all — an offer the app would then refuse is worse than none.
            if crate::rename::replace_allowed(app) {
                banner::RENAME_COLLISION_OPTIONS
            } else {
                &[]
            }
        }
    };

    for opt in options {
        spans.push(Span::styled(opt.label, app.theme.chrome.footer_key));
        spans.push(Span::styled("  ", app.theme.chrome.footer_hint));
    }
    spans.push(Span::styled(
        banner::DIRTY_CLOSE_CANCEL_LABEL,
        app.theme.chrome.footer_hint,
    ));
    spans
}

/// Default-mode hints, now CONTEXTUAL per focused pane (plan WP6.S2,
/// superseding WP2.S6/S7's blind `GLOBAL_BINDINGS` iteration): a
/// priority-ordered `(label, help, active)` list —
///
/// 1. The three held-space leader chords (`global::LEADER_BINDINGS`),
///    always active.
/// 2. `⌘S save` — only in the Editor; styled active only when the
///    document is dirty (assumption A2: always PRESENT there, never
///    removed, so the row never jumps).
/// 3. The focused pane's own table (`EXPLORER_BINDINGS`/`TABS_BINDINGS`) —
///    nothing extra for the Editor, whose chords live in `keymap::resolve`,
///    a match rather than a table (the same asymmetry `help.rs` already
///    records).
/// 4. Every remaining non-alias `GLOBAL_BINDINGS` entry except `⌘S` (already
///    placed at step 2, with its own dirty-state styling) — always last.
///    Aliased ctrl fallbacks (a chord that duplicates a leader chord, or a
///    second quit chord) are skipped: they still work, `help_markdown` still
///    lists them, but the footer only needs to name a command once.
///
/// One source read by both the untruncated renderer (`footer_text`,
/// `rune-fuzz`'s snapshots) and the width-truncated one `draw` uses, so the
/// two can never disagree about WHAT the hints are, only how many fit.
fn default_hint_entries(app: &App) -> Vec<(String, &'static str, bool)> {
    let mut entries: Vec<(String, &'static str, bool)> = LEADER_BINDINGS
        .iter()
        .map(|b| (global::leader_label(b), b.help, true))
        .collect();

    if app.focus == Pane::Editor
        && let Some(save) = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::Save))
    {
        entries.push((save.label(), save.help, app.is_dirty()));
    }

    match app.focus {
        Pane::Explorer => {
            entries.extend(EXPLORER_BINDINGS.iter().map(|b| (b.label(), b.help, true)))
        }
        Pane::Tabs => entries.extend(TABS_BINDINGS.iter().map(|b| (b.label(), b.help, true))),
        // The title field has no binding TABLE of its own — `title::
        // handle_key` matches Enter/Esc/editing keys directly (they are a
        // text field's own behaviour, not chords worth enumerating in the
        // Help doc), so there is nothing here to reflect over. The global
        // hints below still render while renaming.
        Pane::Title => {}
        Pane::Editor => {}
    }

    entries.extend(
        GLOBAL_BINDINGS
            .iter()
            .filter(|b| !b.alias && !matches!(b.cmd, GlobalCommand::Save))
            .map(|b| (b.label(), b.help, true)),
    );

    entries
}

/// One hint entry's own span group: a leading `"  "` separator (every
/// entry but the first), the key label (active/inactive style), a space,
/// then the help text. The chokepoint both `default_hint_spans` (full,
/// untruncated) and `draw`'s width-truncated renderer build from, so an
/// entry can never render differently in the two paths (plan WP6.S3).
fn hint_entry_spans(
    theme: &crate::theme::Theme,
    index: usize,
    label: String,
    help: &'static str,
    active: bool,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if index > 0 {
        spans.push(Span::styled("  ", theme.chrome.footer_hint));
    }
    let key_style = if active {
        theme.chrome.footer_key
    } else {
        theme.chrome.footer_key_inactive
    };
    spans.push(Span::styled(label, key_style));
    spans.push(Span::styled(" ", theme.chrome.footer_hint));
    spans.push(Span::styled(help, theme.chrome.footer_hint));
    spans
}

/// The FULL, untruncated hint spans (plan WP6.S4) — what `footer_text`
/// asserts on and what `rune-fuzz/src/snapshot.rs` captures into every fuzz
/// snapshot. Truncation happens only inside `draw`, so these stay
/// width-independent and the snapshots stay stable.
fn default_hint_spans(app: &App) -> Vec<Span<'static>> {
    default_hint_entries(app)
        .into_iter()
        .enumerate()
        .flat_map(|(i, (label, help, active))| hint_entry_spans(&app.theme, i, label, help, active))
        .collect()
}

/// Priority-truncated hint spans (plan WP6.S3, risk R3): reserves room for
/// `Ln n, Col n` (`right_width`) FIRST, then appends whole entries in
/// priority order only while the next one still fits the remaining width —
/// never a partial entry, so the position readout can never fall off the
/// row.
fn truncated_default_hint_spans(
    app: &App,
    available: usize,
    right_width: usize,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for (i, (label, help, active)) in default_hint_entries(app).into_iter().enumerate() {
        let entry = hint_entry_spans(&app.theme, i, label, help, active);
        let entry_width: usize = entry.iter().map(|s| s.content.chars().count()).sum();
        if used + entry_width + right_width > available {
            break;
        }
        used += entry_width;
        spans.extend(entry);
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
    let bg = app.theme.chrome.footer;
    let right_text = position_text(app);
    let right_width = right_text.chars().count();
    let available = area.width as usize;
    // Truncation (plan WP6.S3/risk R3) only applies to `DefaultHints` — the
    // one mode that grows with the focused pane's own hints; every other
    // mode's content is already short and the three exact-equality tests
    // (`save_error_outranks_everything_else` etc.) depend on it being
    // rendered exactly as `left_spans` produces it, untouched.
    let mut spans = match mode(app) {
        Mode::DefaultHints => truncated_default_hint_spans(app, available, right_width),
        _ => left_spans(app),
    };
    let left_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if available > left_width + right_width {
        spans.push(Span::styled(
            " ".repeat(available - left_width - right_width),
            bg,
        ));
    }
    spans.push(Span::styled(right_text, app.theme.chrome.footer_meta));
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
        // Editor focus (`App::new`'s default) — every non-alias
        // `GLOBAL_BINDINGS` help string must still appear, though
        // `explorer`/`editor`/`tabs` now come from the leader table's own
        // entries rather than `GLOBAL_BINDINGS` itself. Aliased bindings
        // (like `^d`'s "quit") are excluded on purpose: their help string is
        // shared with a non-alias binding (`^c`'s "quit"), so iterating the
        // full table would still pass even if the alias filter broke — this
        // must walk only `!b.alias` entries to actually test that filter.
        let app = app_with("hello");
        assert_eq!(app.focus, Pane::Editor);
        let text = footer_text(&app);
        for binding in GLOBAL_BINDINGS.iter().filter(|b| !b.alias) {
            assert!(
                text.contains(binding.help),
                "expected {:?} in default footer text {text:?}",
                binding.help
            );
        }
    }

    /// Plan WP6.S5: with Explorer focus, the pane's own keys show and
    /// `save` (Editor-only, assumption A2) does not.
    #[test]
    fn explorer_focus_shows_its_own_keys_and_omits_save() {
        let mut app = app_with("hello");
        app.focus = Pane::Explorer;
        let text = footer_text(&app);
        assert!(text.contains("up dir"), "footer text: {text:?}");
        assert!(!text.contains("save"), "footer text: {text:?}");
    }

    /// Plan WP6.S5: with Tabs focus, the pane's own keys show.
    #[test]
    fn tabs_focus_shows_its_own_keys() {
        let mut app = app_with("hello");
        app.focus = Pane::Tabs;
        let text = footer_text(&app);
        assert!(text.contains("close"), "footer text: {text:?}");
    }

    /// Plan WP6.S6 — the span-level regression guard for assumption A2: the
    /// `⌘S` label span carries `theme.chrome.footer_key_inactive` on a
    /// clean document and `theme.chrome.footer_key` once an edit makes it
    /// dirty. Reads `default_hint_spans` directly (private, but visible
    /// from this descendant module) rather than re-deriving the span list.
    #[test]
    fn save_label_span_is_styled_inactive_when_clean_and_active_when_dirty() {
        let save_label = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::Save))
            .expect("a Save binding exists")
            .label();

        let app = app_with("hello");
        assert!(!app.is_dirty());
        let spans = default_hint_spans(&app);
        let save_span = spans
            .iter()
            .find(|s| s.content.as_ref() == save_label)
            .expect("save label span present");
        assert_eq!(save_span.style, app.theme.chrome.footer_key_inactive);

        let mut app = app_with("hello");
        app.active_doc_mut().is_dirty_cached = true;
        assert!(app.is_dirty());
        let spans = default_hint_spans(&app);
        let save_span = spans
            .iter()
            .find(|s| s.content.as_ref() == save_label)
            .expect("save label span present");
        assert_eq!(save_span.style, app.theme.chrome.footer_key);
    }

    /// The Guard mode's rendered labels are exactly `banner::DIRTY_CLOSE_
    /// OPTIONS`/`DIRTY_CLOSE_CANCEL_LABEL` — the same consts `banner::
    /// handle_guard_key` matches its `s`/`d` keys against (review fix: no
    /// more independently hand-maintained literal here).
    #[test]
    fn guard_mode_labels_come_from_the_shared_dirty_close_consts() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.modal = Some(crate::banner::Modal::Guard(crate::banner::GuardPrompt {
            doc,
            kind: crate::banner::GuardKind::DirtyClose,
        }));

        let text = footer_text(&app);
        for opt in crate::banner::DIRTY_CLOSE_OPTIONS {
            assert!(
                text.contains(opt.label),
                "expected {:?} in the Guard footer text {text:?}",
                opt.label
            );
        }
        assert!(text.contains(crate::banner::DIRTY_CLOSE_CANCEL_LABEL));
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
