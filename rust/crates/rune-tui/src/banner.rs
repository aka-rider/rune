//! The modal banner (plan WP3, decision 13): `App.modal` capture-stage
//! handling, the private read-only `Document` an `ErrorState` owns, and the
//! banner's own render pipeline — a thin cell-building wrapper around
//! `render::segment_cells`/`render::blit` scoped to the MODAL document
//! instead of `app.active_doc()` (`render.rs::build_rows` is hardcoded to
//! the active document, so it can't be reused directly here without
//! widening its signature past what plan WP3.S3's "keep render.rs edits
//! MINIMAL" rule allows).
//!
//! `Modal` is a single `Option` field on `App` (plan Risks, "Banner
//! reentrancy"): `set_modal` is the ONE chokepoint that decides whether a
//! new modal replaces whatever's already up, by priority — `Error` is the
//! only variant today, but the match in `Modal::priority` is where WP5's
//! `Guard` variant slots in under it, so that later addition never needs a
//! second, independently-written priority rule to drift from this one.

use ratatui::Frame;
use ratatui::layout::Rect;

use rune_core::buffer::Buffer;

use crate::app::App;
use crate::clipboard::osc52_copy;
use crate::document::Document;
use crate::keymap::{KeyCode, KeyInput};
use crate::render::{self, Cell};
use crate::runtime::Effects;

/// One modal state `App.modal` can hold — `Option<Modal>`, never a stack:
/// only ever the single highest-priority modal currently warranted (see
/// `set_modal`). `Error` is WP3's only variant; WP5 adds `Guard` beneath it.
pub enum Modal {
    Error(ErrorState),
}

impl Modal {
    /// Higher wins ties (`set_modal`'s `>=`): a second `Error` while an
    /// `Error` is already up REPLACES it (plan Risks: a fresh error is
    /// never suppressed by a stale one) rather than being dropped. WP5's
    /// `Guard` arm returns a value below this one, so an `Error` raised
    /// while a `Guard` is up always wins, but a `Guard` raised while an
    /// `Error` is up does not silently displace it.
    fn priority(&self) -> u8 {
        match self {
            Modal::Error(_) => 10,
        }
    }

    /// The bytes `'c'`/`'C'` (stage-1 key capture) copies via OSC 52 — the
    /// modal document's whole buffer content, byte-verbatim.
    fn copy_bytes(&self) -> Vec<u8> {
        match self {
            Modal::Error(state) => state.doc.buffer.content().as_bytes().to_vec(),
        }
    }

    /// The synced modal document's total wrap-row count — the height input
    /// `render::draw` needs to size the banner `Rect` (plan WP3.S3:
    /// "`DisplaySnapshot.total_rows` gives the height input"). `0` before
    /// the modal's first `sync_modal` pass (never observed in practice:
    /// `App::sync_view` syncs it the same tick `set_modal` raises it, before
    /// the next `render::draw`).
    pub fn total_rows(&self) -> usize {
        match self {
            Modal::Error(state) => state
                .doc
                .view
                .as_ref()
                .map(|v| v.display.total_rows)
                .unwrap_or(0),
        }
    }
}

/// The banner's private state (plan WP3.S1): a read-only `Document` that is
/// NOT in `App.documents` and has no tab — `render::draw`'s editor-area
/// blit and every doc-scoped command (`commands::nav`/`commands::edit`) never
/// see it, since neither is ever handed its id (it has none).
pub struct ErrorState {
    pub doc: Document,
}

impl ErrorState {
    /// Builds the banner's markdown buffer: `⚠ <headline>`, a blank line,
    /// then the rest of `text` VERBATIM — unwrapped, untruncated, exactly
    /// as given (plan WP3.S1). `headline` is `text`'s own first line, so a
    /// single-line error (no `\n` at all) shows just its headline with an
    /// empty body below, never a duplicated copy of the same line.
    pub fn new(text: &str) -> ErrorState {
        let mut parts = text.splitn(2, '\n');
        let headline = parts.next().unwrap_or("");
        let body = parts.next().unwrap_or("");
        let markdown = format!("\u{26A0} {headline}\n\n{body}");

        let mut doc = Document::new(Buffer::new(markdown));
        // `db: None`/`file_path: None` already hold — `Document::new`'s
        // defaults (plan WP3.S1: constructed read-only, no store binding,
        // no file identity).
        doc.read_only = true;
        // Never focused: this document is never the stage-3 editor pane,
        // so it should always render fully rendered/concealed (Gotchas:
        // "Unfocused -> ForceRendered") regardless of `App.focus`.
        doc.focused = false;
        ErrorState { doc }
    }
}

/// The priority chokepoint (plan Risks, "Banner reentrancy"): a NEW modal
/// replaces whatever's currently up only if it outranks (or ties) it —
/// never silently dropped, never silently downgraded. The one and only
/// writer of `App.modal` besides stage-1 key handling below (`Esc`/`c`
/// clearing it) — every caller that wants to raise a modal goes through
/// this or `report_error`, never `app.modal = Some(...)` directly.
pub fn set_modal(app: &mut App, modal: Modal) {
    let should_replace = app
        .modal
        .as_ref()
        .is_none_or(|existing| modal.priority() >= existing.priority());
    if should_replace {
        app.modal = Some(modal);
    }
}

/// The chokepoint every error-reporting call site routes through (plan
/// WP3.S1/S4) instead of writing `app.status_message` directly — a full
/// banner instead of a footer-line message.
pub fn report_error(app: &mut App, text: impl Into<String>) {
    set_modal(app, Modal::Error(ErrorState::new(&text.into())));
}

/// Re-syncs the modal's private `Document` at `width` and caches the
/// resulting view (plan WP3.S3), mirroring what `App::sync_view` already
/// does for the active document — called from there, once per frame,
/// BEFORE `render::draw` reads `Modal::total_rows` to size the banner
/// `Rect`. Keeping this mutation in the settle step (not inside `render`
/// itself, which only ever borrows `&App`) is what keeps every synchronous
/// state write inside `update`/its settle phase, never inside rendering
/// (§5.4).
pub fn sync_modal(app: &mut App, width: u16) {
    let Some(modal) = app.modal.as_mut() else {
        return;
    };
    match modal {
        Modal::Error(state) => {
            state.doc.viewport.width = width;
            let view = state.doc.sync();
            state.doc.view = Some(view);
        }
    }
}

/// Stage 1 of the four-stage key pipeline (plan Context, decision 8;
/// `app::handle_key`'s insertion point): while `app.modal` is `Some`, EVERY
/// key is consumed HERE — quit chords included, a modal interposes on quit
/// by design (plan WP3.S2) — never falling through to stage 2/3. `Esc`
/// clears the modal; `c`/`C` copies the modal document's whole buffer via
/// OSC 52 then clears it; `Up`/`Down`/`PageUp`/`PageDown` scroll it;
/// everything else is a consumed no-op.
pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    match key.code {
        KeyCode::Escape => app.modal = None,
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&'c') => {
            if let Some(modal) = &app.modal {
                effects.raw.push(osc52_copy(&modal.copy_bytes()));
            }
            app.modal = None;
        }
        KeyCode::Up => scroll(app, -1),
        KeyCode::Down => scroll(app, 1),
        KeyCode::PageUp => scroll(app, -page_amount(app)),
        KeyCode::PageDown => scroll(app, page_amount(app)),
        _ => {}
    }
}

/// The modal document's own viewport height, as a page-scroll amount — `1`
/// when there's no modal up at all (dead in practice: every caller already
/// checked `app.modal.is_some()`).
fn page_amount(app: &App) -> isize {
    match &app.modal {
        Some(Modal::Error(state)) => state.doc.viewport.height.max(1) as isize,
        None => 1,
    }
}

/// Moves the modal document's `viewport.scroll_row` by `delta` wrap rows,
/// clamped to `[0, total_rows - 1]` once it has a synced view (before that,
/// only the lower bound applies) — the minimal scroll mechanics plan WP3.S2
/// asks for, reusing `Viewport::scroll_row` rather than inventing a second
/// scroll-position field.
fn scroll(app: &mut App, delta: isize) {
    let Some(Modal::Error(state)) = app.modal.as_mut() else {
        return;
    };
    let max_row = state
        .doc
        .view
        .as_ref()
        .map(|v| v.display.total_rows.saturating_sub(1))
        .unwrap_or(usize::MAX);
    let current = state.doc.viewport.scroll_row as isize;
    let next = (current + delta).max(0) as usize;
    state.doc.viewport.scroll_row = next.min(max_row);
}

/// The banner's own row-building step (plan WP3.S3: "ALL cell building
/// lives in banner.rs"): the modal document's wrap segments from
/// `scroll_row` for up to `height` rows, through the same
/// `render::segment_cells` every editor row goes through. No cursor/
/// selection overlay — the banner document's `CursorSet` is never driven by
/// any command (stage 1 never touches it), so there is nothing meaningful
/// to overlay.
fn build_rows(doc: &Document, height: u16) -> Vec<Vec<Cell>> {
    let Some(view) = &doc.view else {
        return Vec::new();
    };
    view.wrap
        .segments()
        .iter()
        .skip(doc.viewport.scroll_row)
        .take(height as usize)
        .map(render::segment_cells)
        .collect()
}

/// Renders the banner into `area` — the SOLE entry point `render::draw`
/// calls (plan WP3.S3's "(b) ONE call `banner::draw(...)`"), reached only
/// once `render.rs` has already sized `area` from `Modal::total_rows`.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let Some(modal) = &app.modal else {
        return;
    };
    match modal {
        Modal::Error(state) => {
            let rows = build_rows(&state.doc, area.height);
            render::blit(&rows, area, frame);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn error_state_puts_the_first_line_as_the_headline_and_the_rest_as_body() {
        let state = ErrorState::new("boom\n\nsecond line\nthird line");
        // `text`'s own first line becomes the headline; EVERYTHING after
        // its first `\n` — including the blank line already present in
        // this input — is the body, carried verbatim (plan WP3.S1).
        assert_eq!(
            state.doc.buffer.content(),
            "\u{26A0} boom\n\n\nsecond line\nthird line"
        );
    }

    #[test]
    fn error_state_single_line_has_an_empty_body() {
        let state = ErrorState::new("boom");
        assert_eq!(state.doc.buffer.content(), "\u{26A0} boom\n\n");
    }

    #[test]
    fn error_state_is_read_only_and_unbound() {
        let state = ErrorState::new("boom");
        assert!(state.doc.read_only);
        assert!(state.doc.file_path.is_none());
        assert!(state.doc.db.is_none());
        assert!(!state.doc.focused);
    }

    #[test]
    fn set_modal_replaces_an_equal_or_lower_priority_modal() {
        let mut app = crate::app::App::new(
            rune_core::buffer::Buffer::new("hi"),
            None,
            std::sync::Arc::new(rune_vfs::Mem::new()),
            None,
        );
        set_modal(&mut app, Modal::Error(ErrorState::new("first")));
        set_modal(&mut app, Modal::Error(ErrorState::new("second")));
        match app.modal {
            Some(Modal::Error(state)) => {
                assert!(state.doc.buffer.content().contains("second"));
            }
            None => panic!("expected a modal to be set"),
        }
    }

    #[test]
    fn report_error_sets_the_modal() {
        let mut app = crate::app::App::new(
            rune_core::buffer::Buffer::new("hi"),
            None,
            std::sync::Arc::new(rune_vfs::Mem::new()),
            None,
        );
        report_error(&mut app, "boom");
        assert!(app.modal.is_some());
    }
}
