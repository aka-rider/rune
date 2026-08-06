//! The footer row: a display-mode priority renderer, pure function of
//! `&App` (plan WP2.S6): the `Mode` enum's declaration order below IS
//! priority order, highest first. File
//! renamed from `status.rs`: WP1's per-doc file-name/dirty-dot display
//! moves to WP6's `title.rs`; this module owns only the chrome-wide message
//! row + the always-visible cursor position.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::footer_hints::{default_hint_spans, truncated_default_hint_spans};
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::pane::Pane;
use crate::width::display_width;
use rune_syntax::wrap::line_visual_col;

/// Which single visual state the footer's left side shows for this render
/// — priority order highest-first: the Guard prompt always wins, then the
/// messages pane's own keys (only while it holds focus), then a pending
/// chord hint, the degraded-store
/// banner, the merge/disk-changed ambient hints, and the default keymap
/// hints. Mutually exclusive by construction — never concatenated, unlike
/// the pre-WP2 `status.rs::status_text`. Transient user-facing messages no
/// longer have a `Mode` of their own: they live in the message log/pane
/// (`messages`) instead.
enum Mode<'a> {
    /// The close/quit/rename/disk-conflict confirmation prompt. Carries the
    /// whole `GuardPrompt`, not just its `GuardKind`: `guard_spans` needs
    /// `prompt.doc` to name WHICH document a `DirtyQuit` prompt is blocking
    /// on — a `GuardKind` alone can't say that.
    Guard(&'a GuardPrompt),
    /// The messages pane's own hint row — entered whenever the pane holds
    /// focus, ranked directly below `Guard` and above every
    /// other mode: while the user is inside the pane, its own keys
    /// (`[⌘C] copy`, `[Esc] close`) are the only thing worth showing.
    Messages,
    ChordPending(String),
    Degraded(&'a str),
    /// Passive persistent hint (plan WP2.S5, Assumption A1): the ACTIVE
    /// document's disk fact has diverged from the buffer. Ranked just below
    /// `Degraded` — the message log carries every actual status text now,
    /// so this ambient reminder only needs to outrank the bare default
    /// keymap hints.
    DiskChanged,
    /// Persistent resolver reminder (plan WP4.S4) carrying the live
    /// unresolved count.
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
    // Review fix F5: gated on the merge doc being the ACTIVE one, same as
    // `merge/keys.rs`'s own intercept — otherwise switching to a different
    // tab mid-merge (before the auto-exit below ever runs, or on a path
    // that leaves a stale reference) would keep showing "[O]urs [T]heirs"
    // hints for a document that isn't even on screen.
    if let crate::merge::MergeState::Active { doc, .. } = app.merge
        && doc == app.active
    {
        return Mode::MergeHint(app.merge.unresolved_count());
    }
    // Suppressed while a merge attempt is underway (plan WP4.S4): `Active`
    // returned above, and a `Pending` attempt's "[^M]erge" invitation
    // would be stale advice about the very thing already in flight.
    if matches!(app.merge, crate::merge::MergeState::Inactive)
        && matches!(
            app.active_doc().last_sync,
            Some(rune_db::SyncKind::DiskAhead) | Some(rune_db::SyncKind::Diverged)
        )
    {
        return Mode::DiskChanged;
    }
    Mode::DefaultHints
}

/// `pending_quit`'s hint has no other home (the pre-WP2 `quit_hint`, ported
/// unchanged below); `pending_save_confirm`'s hint is the fixed literal
/// below — `trigger_save` also posts the degraded-save explanation to the
/// message log the same tick it arms the confirm gate, so this no longer
/// needs to read that text back out of a shared slot. The
/// `cid == app.active` check is load-bearing: `pending_save_confirm` is
/// doc-tagged (armed for the document that attempted the save), so
/// switching tabs away from that document must not leave its stale hint
/// showing over whatever document is active now.
fn chord_hint(app: &App) -> Option<String> {
    if app.pending_quit.is_some() {
        return Some(quit_hint(app).to_string());
    }
    if app
        .pending_save_confirm
        .is_some_and(|(cid, _)| cid == app.active)
    {
        return Some("press \u{2318}S again to save anyway".to_string());
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
        Mode::Guard(prompt) => guard_spans(app, prompt),
        Mode::Messages => vec![
            Span::styled("[\u{2318}C] copy", app.theme.chrome.footer_key),
            Span::styled("  ", app.theme.chrome.footer_hint),
            Span::styled("[Esc] close", app.theme.chrome.footer_hint),
        ],
        Mode::ChordPending(text) => vec![Span::styled(text, app.theme.chrome.footer_key)],
        Mode::Degraded(msg) => vec![Span::styled(msg.to_string(), app.theme.chrome.footer_hint)],
        Mode::DiskChanged => vec![Span::styled(
            "\u{21c4} disk changed \u{2014} [^M]erge",
            app.theme.chrome.footer_hint,
        )],
        Mode::MergeHint(unresolved) => vec![Span::styled(
            format!(
                "\u{2699} merge \u{2014} [O]urs [T]heirs [B]oth · [ ] navigate · {unresolved} left"
            ),
            app.theme.chrome.footer_hint,
        )],
        Mode::DefaultHints => default_hint_spans(app),
    }
}

/// The dirty-close/dirty-quit Guard's `[S]ave [D]iscard [Esc] Cancel` hint,
/// built from `guard::DIRTY_CLOSE_OPTIONS`/`DIRTY_CLOSE_CANCEL_LABEL` — the
/// SAME consts `guard::handle_guard_key` matches its `s`/`d` keys against,
/// so this render can
/// never drift from what those keys actually do (review fix).
fn guard_spans(app: &App, prompt: &GuardPrompt) -> Vec<Span<'static>> {
    let mut spans = Vec::new();

    // A rename collision names its target: "replace <what>?" is a question
    // the user can answer; a bare `[R]eplace` is not. `DirtyClose`/
    // `DirtyQuit` both name WHICH document is waiting — `DirtyQuit` for the
    // original reason (plan WP2: "a background dirty document blocks quit
    // with no hint which one"), and `DirtyClose` because the tab-cap
    // eviction can now arm it for a document that was not the active one a
    // moment ago, so the prompt must say which buffer it covers just as
    // plainly. Both read the name via `Document::file_name`, the same name
    // the tab bar shows, so the user matches the prompt against something
    // already on screen. Deliberately NOT `title::name_for`: that one
    // answers "what should the title FIELD hold", which for a pathless
    // draft is the editable `.md` stub — a prompt reading "unsaved changes
    // in .md" names nothing at all.
    let options: &[guard::GuardOption] = match &prompt.kind {
        GuardKind::DirtyClose | GuardKind::DirtyQuit => {
            let name = app
                .doc(prompt.doc)
                .map(|doc| doc.file_name().to_string())
                .unwrap_or_default();
            spans.push(Span::styled(
                format!("unsaved changes in {name} \u{2014} "),
                app.theme.chrome.footer_hint,
            ));
            guard::DIRTY_CLOSE_OPTIONS
        }
        GuardKind::RenameCollision { target } => {
            spans.push(Span::styled(
                format!("{target} already exists  "),
                app.theme.chrome.footer_hint,
            ));
            // Without a durable store there is nowhere to preserve
            // the replaced file's bytes, so the option is not offered at
            // all — an offer the app would then refuse is worse than none.
            if crate::rename::replace_allowed(app) {
                guard::RENAME_COLLISION_OPTIONS
            } else {
                &[]
            }
        }
        GuardKind::DiskConflict { .. } => {
            let name = app
                .doc(prompt.doc)
                .map(|doc| doc.file_name().to_string())
                .unwrap_or_default();
            spans.push(Span::styled(
                format!("{name} changed on disk \u{2014} "),
                app.theme.chrome.footer_hint,
            ));
            guard::DISK_CONFLICT_OPTIONS
        }
        GuardKind::Trash { path } => {
            let name = crate::trash::display_name(path);
            spans.push(Span::styled(
                format!("Trash {name}? "),
                app.theme.chrome.footer_hint,
            ));
            guard::TRASH_OPTIONS
        }
    };

    for opt in options {
        spans.push(Span::styled(opt.label, app.theme.chrome.footer_key));
        spans.push(Span::styled("  ", app.theme.chrome.footer_hint));
    }
    spans.push(Span::styled(
        guard::DIRTY_CLOSE_CANCEL_LABEL,
        app.theme.chrome.footer_hint,
    ));
    spans
}

/// The footer's left-side text content, with no styling — for tests that
/// only need to assert on WHAT shows, not how (mirrors the pre-WP2
/// `status_text`'s role).
pub fn footer_text(app: &App) -> String {
    left_spans(app).iter().map(|s| s.content.as_ref()).collect()
}

/// The persistent diverged marker beside the position readout: `"⇄ "`
/// while the ACTIVE document's last known sync classification says the
/// disk holds changes the buffer doesn't (`DiskAhead`/`Diverged`), empty
/// otherwise. `BufferAhead` is deliberately excluded — that's the dirty
/// flag's job. Unlike the mutually-exclusive left-side `Mode`s, this rides
/// the always-rendered right side, so it survives Guard/Messages/Chord
/// modes shadowing the `DiskChanged` banner.
fn sync_marker(app: &App) -> &'static str {
    match app.active_doc().last_sync {
        Some(rune_db::SyncKind::DiskAhead) | Some(rune_db::SyncKind::Diverged) => "\u{21c4} ",
        _ => "",
    }
}

/// `Ln <line>, Col <col>` from the active document's primary cursor (plan
/// WP8) — always shown, regardless of the left side's `Mode`. Col is a LINE-relative
/// terminal-CELL column (`rune_syntax::wrap::line_visual_col`), not a
/// wrap-row-relative one: `line_visual_col` walks the cursor's own logical
/// line from its own start, so a wrapped line's second-and-later visual row
/// never resets the readout back to `Col 1`.
pub fn position_text(app: &App) -> String {
    let doc = app.active_doc();
    let offset = doc.cursors.primary().position;
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
    // Truncation (plan WP6.S3/risk R3) only applies to `DefaultHints` — the
    // one mode that grows with the focused pane's own hints; every other
    // mode's content is already short and the three exact-equality tests
    // (`save_error_outranks_everything_else` etc.) depend on it being
    // rendered exactly as `left_spans` produces it, untouched.
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

    /// Review fix F5: `MergeHint` is gated on the merge doc being the
    /// ACTIVE one, the same check `merge/keys.rs::intercept` uses — a
    /// merge `Active` on some OTHER (not-currently-shown) document must
    /// not paint "[O]urs [T]heirs" hints over it.
    #[test]
    fn merge_hint_is_suppressed_when_the_merge_document_is_not_active() {
        let mut app = app_with("hello");
        let merge_doc = app.active;
        let other = app.open_document(Buffer::new("scratch"));
        app.active = other;
        app.merge = crate::merge::MergeState::Active {
            doc: merge_doc,
            conflicts: Vec::new(),
            blocks: Vec::new(),
            cur: 0,
            saved_display_name: None,
        };

        let text = footer_text(&app);
        assert!(
            !text.contains('⚙') && !text.contains("[O]urs"),
            "merge hint leaked onto an inactive document's footer: {text:?}"
        );
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
        assert_eq!(app.focus(), Pane::Editor);
        let text = footer_text(&app);
        for binding in GLOBAL_BINDINGS.iter().filter(|b| !b.alias) {
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

    /// Plan WP6.S5: with Explorer focus, the pane's own keys show and
    /// `save` (Editor-only, assumption A2) does not.
    #[test]
    fn explorer_focus_shows_its_own_keys_and_omits_save() {
        let mut app = app_with("hello");
        // Focus is gated on `LayoutMode` — the pane must actually be
        // painted (`App::new`'s default left column starts hidden) before
        // `set_focus_pane` will land on it instead of falling back to the
        // Editor.
        app.splits.left.show();
        app.set_focus_pane(Pane::Explorer, &mut crate::runtime::Effects::default());
        let text = footer_text(&app);
        assert!(text.contains("up dir"), "footer text: {text:?}");
        assert!(!text.contains("save"), "footer text: {text:?}");
    }

    /// Plan WP6.S5: with Tabs focus, the pane's own keys show.
    #[test]
    fn tabs_focus_shows_its_own_keys() {
        let mut app = app_with("hello");
        app.splits.left.show();
        app.set_focus_pane(Pane::Tabs, &mut crate::runtime::Effects::default());
        let text = footer_text(&app);
        assert!(text.contains("close"), "footer text: {text:?}");
    }

    /// The Guard mode's rendered labels are exactly `guard::DIRTY_CLOSE_
    /// OPTIONS`/`DIRTY_CLOSE_CANCEL_LABEL` — the same consts `guard::
    /// handle_guard_key` matches its `s`/`d` keys against (review fix: no
    /// more independently hand-maintained literal here).
    #[test]
    fn guard_mode_labels_come_from_the_shared_dirty_close_consts() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.guard = Some(crate::guard::GuardPrompt {
            doc,
            kind: crate::guard::GuardKind::DirtyClose,
        });

        let text = footer_text(&app);
        for opt in crate::guard::DIRTY_CLOSE_OPTIONS {
            assert!(
                text.contains(opt.label),
                "expected {:?} in the Guard footer text {text:?}",
                opt.label
            );
        }
        assert!(text.contains(crate::guard::DIRTY_CLOSE_CANCEL_LABEL));
    }

    #[test]
    fn pending_quit_shows_the_quit_hint_over_the_degraded_banner() {
        let mut app = app_with("hello");
        app.db_banner = Some("recovery disabled: boom".to_string());
        app.pending_quit = Some((QuitKey::CtrlC, 0));
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

    /// Renders the full footer row through `testgrid::draw_with` (the
    /// crate's one `TestBackend` construction site) and returns its text —
    /// `footer_text` covers only the left side, and the ⇄ marker rides the
    /// right side beside the position readout.
    fn footer_row(app: &App, width: u16) -> String {
        let buf = crate::testgrid::draw_with(width, 1, |frame| {
            draw(app, Rect::new(0, 0, width, 1), frame)
        });
        (0..width)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()))
            .collect()
    }

    /// The persistent ⇄ marker sits on the footer's right side, directly
    /// before the position readout, for `DiskAhead`/`Diverged` — and never
    /// for a clean or merely dirty (`BufferAhead`) document. Asserted on
    /// the `"⇄ Ln"` juxtaposition, not a bare `contains('⇄')`: the
    /// `DiskChanged` left-side banner uses the same glyph.
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
