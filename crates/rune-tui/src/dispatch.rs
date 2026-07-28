//! The `Msg` dispatcher, key pipeline, and `rune-db` event router, split out
//! of `app` (§1.6 budget): `update_inner` is the top-level `Msg` match
//! `app::update` used to run inline; its `Msg::Key`/`Msg::Db` arms route to
//! the four-stage key pipeline and the `rune-db` ack router right below.
//! Nothing else changes — every one of these is exactly the function
//! `app.rs` used to define locally, now reached through `dispatch::`
//! instead.

use std::ops::Range;

use crate::app::App;
use crate::commands::{clipboard, edit, edit_lines, mouse, multi, nav, nav_scroll};
use crate::document::{Document, DocumentId};
use crate::keymap::{self, Command, KeyCode, KeyInput, Mods, QuitKey};
use crate::pane::{self, Pane};
use crate::runtime::{Effects, Msg};
use crate::{explorer, opentabs, save};
use rune_db::DbEvent;
use rune_syntax::ScopeId;

/// The one dispatcher every `Msg` funnels through (`app::update`'s inner
/// half, split out here alongside the key/db-event routers it calls into —
/// §1.6 budget). `app::update` wraps this with the snapshot-autosave
/// debounce chokepoint; nothing else in the crate calls this directly.
pub(crate) fn update_inner(app: &mut App, msg: Msg, effects: &mut Effects) {
    match msg {
        Msg::Key(key) => handle_key(app, key, effects),
        Msg::Mouse(input) => mouse::handle(app, input),
        Msg::Resize(width, height) => {
            app.frame_width = width;
            app.frame_height = height;
            app.relayout();
        }
        Msg::Paste(text) => {
            // Bracketed paste and pbpaste (`Msg::ClipboardRead` below) both
            // funnel through the same `handle_paste_content` (plan Gotchas:
            // "Bracketed paste vs pbpaste double-paste" — never handle one
            // event twice, never insert through two different paths).
            clipboard::handle_paste_content(app, app.active, &text);
        }
        Msg::ClipboardRead(text) => {
            clipboard::handle_paste_content(app, app.active, &text);
        }
        Msg::SaveDone {
            id,
            version,
            result,
        } => save::handle_save_done(app, id, version, result),
        Msg::ConfirmTimeout { generation } => {
            if let Some((_, pending_gen)) = app.pending_quit
                && pending_gen == generation
            {
                app.pending_quit = None;
            }
            // A stale generation (the user already quit-confirmed or
            // re-armed with a new chord since) is ignored.
        }
        Msg::SaveConfirmTimeout { generation } => {
            if app
                .pending_save_confirm
                .is_some_and(|(_, g)| g == generation)
            {
                app.pending_save_confirm = None;
            }
        }
        Msg::SnapshotDue { id, generation } => save::handle_snapshot_due(app, id, generation),
        Msg::Db(evt) => handle_db_event(app, evt, effects),
        Msg::DirLoaded {
            root,
            entries,
            cause,
            generation,
        } => explorer::handle_dir_loaded(app, root, entries, cause, generation),
        // Routed through the modal banner, not `status_message` (plan
        // WP3.S4) — `report_error` is the one chokepoint every error
        // report funnels through.
        Msg::RenameDone { generation, result } => {
            crate::rename::handle_rename_done(app, generation, result, effects)
        }
        Msg::Highlighted {
            doc,
            version,
            result,
        } => handle_highlighted(app, doc, version, result, effects),
        Msg::HighlightRetried {
            doc,
            version,
            result,
        } => handle_highlight_retried(app, doc, version, result, effects),
        Msg::Error(e) => crate::banner::report_error(app, e),
        Msg::Quit => {
            app.should_quit = true;
        }
    }
}

/// Routes a `rune-db` writer-thread completion (plan WP5.S1, re-routed in
/// WP1 via `App::db_ops` — plan decision 6): the ack's own op id is popped
/// from `db_ops` to find which `DocumentId` enqueued it; an id with no
/// entry (already resolved, or from a `Load` op handled during bootstrap
/// hydration instead — see `db::DbBridge`'s doc comment) is ignored. Only
/// `Materialize` acks (the save path, WP5.S6) and `AppendEdit` acks (seq
/// bookkeeping, `db::resolve_append_ack`) need a per-document reaction on
/// success; `MoveUndoPos`/`CreateSnapshot`/adoption acks are fire-and-
/// forget. Any `Err`/`Fatal` degrades the WHOLE store (plan decision 3) —
/// never a buffer rollback.
pub(crate) fn handle_db_event(app: &mut App, evt: DbEvent, effects: &mut Effects) {
    match evt {
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Seq(seq),
        } => {
            if let Some(doc_id) = app.db_ops.remove(&op_id) {
                crate::db::resolve_append_ack(app, doc_id, seq);
            }
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Materialize(mat),
        } => {
            if let Some(doc_id) = app.db_ops.remove(&op_id) {
                save::handle_materialize_ack(app, doc_id, *mat);
            }
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Rename(outcome),
        } => {
            app.db_ops.remove(&op_id);
            crate::rename::handle_rename_ack(app, op_id, *outcome, effects);
        }
        DbEvent::Ok { id: op_id, .. } => {
            app.db_ops.remove(&op_id);
        }
        DbEvent::Err { id: op_id, error } => {
            app.db_ops.remove(&op_id);
            save::on_store_failure(app, error);
        }
        DbEvent::Fatal { error } => {
            save::on_store_failure(app, error);
            // Degraded mode gates every FUTURE enqueue (`db::append_edit`/
            // `move_undo_pos`/`save::materialize_now`/`handle_snapshot_due`
            // all bail out once `db.degraded`), but does nothing about
            // in-flight entries already sitting in `db_ops` — a `Fatal`
            // tears the whole writer thread down, so none of them will
            // EVER receive their ack. Left alone, they'd carry dead weight
            // forward for the rest of the session (an unbounded leak across
            // a long-running degrade-then-keep-editing session); clearing
            // them here is correct, not just tidy — `App::doc_mut` already
            // treats a missing `db_ops` entry as a plain no-op for any
            // ack that *did* somehow still land, so no real ack is ever
            // silently dropped by this.
            app.db_ops.clear();
        }
    }
}

/// The span-clamp chokepoint `handle_highlighted` and `handle_highlight_
/// retried` (finding B) both apply on a reply whose spans are to be
/// accepted: every range clamped to the live byte length, with mid-`char`
/// or inverted ranges (§1.3) discarded, and the survivors replace `spans`
/// tagged with the version they describe.
fn apply_highlight_spans(doc: &mut Document, version: u64, spans: Vec<(Range<usize>, ScopeId)>) {
    let content = doc.buffer.content();
    let len = content.len();
    let mut clamped: Vec<(Range<usize>, ScopeId)> = Vec::with_capacity(spans.len());
    for (range, scope) in spans {
        let start = range.start;
        let end = range.end.min(len);
        if start >= end || !content.is_char_boundary(start) || !content.is_char_boundary(end) {
            continue;
        }
        clamped.push((start..end, scope));
    }
    doc.highlight.spans = clamped;
    doc.highlight.version = version;
}

/// Applies a `Msg::Highlighted` reply (plan WP5.S4), in the fixed order
/// `[R2]` requires: (a) `in_flight` clears regardless of what the reply
/// carries, so a document can never deadlock waiting on a highlight that
/// already returned; (b) `result: None` (budget elapsed, unknown language,
/// parse failure) leaves `spans` exactly as they were — a slow document
/// degrades to STALE colours, never to none; (c) a `version` that no longer
/// matches the live buffer means a NEWER edit landed while this reply was in
/// flight, so the payload describes stale content and is dropped, spans
/// again left untouched; (d) otherwise `apply_highlight_spans` clamps and
/// stores the survivors; (e) if a further edit arrived while this reply was
/// in flight (`pending`), it is cleared and a fresh highlight is requested
/// immediately rather than waiting for the next keystroke.
///
/// Finding B's one exception to (b): a document that has NEVER been
/// highlighted (`doc.highlight.version == 0`, `Buffer::version` never being
/// 0 itself) has no previous spans for `[R2]` to fall back on, so `result:
/// None` at the live version would otherwise leave it silently,
/// permanently uncoloured — nothing else re-schedules a highlight for an
/// unedited document. `highlight::retry_highlight` gives it exactly one
/// further attempt at a widened budget instead; see that function's and
/// `Msg::HighlightRetried`'s doc comments for why this is bounded.
fn handle_highlighted(
    app: &mut App,
    id: DocumentId,
    version: u64,
    result: Option<rune_ts::HighlightResult>,
    effects: &mut Effects,
) {
    let mut retry = false;
    let mut pending = false;
    if let Some(doc) = app.doc_mut(id) {
        doc.highlight.in_flight = None;
        pending = doc.highlight.pending;
        doc.highlight.pending = false;
        let live_version = doc.buffer.version();
        match result {
            Some(spans) if version == live_version => {
                doc.highlight.truncated = spans.truncated;
                apply_highlight_spans(doc, version, spans.spans);
            }
            None if version == live_version && doc.highlight.version == 0 => {
                retry = true;
            }
            // `result: None` on a document with existing spans, or `Some`/
            // `None` at a stale version, leaves `spans` untouched — `[R2]`.
            _ => {}
        }
    }

    if retry {
        crate::highlight::retry_highlight(app, id, version, effects);
        return;
    }

    if pending {
        crate::highlight::schedule_highlight(app, id, effects);
    }
}

/// Applies `Msg::HighlightRetried` (finding B's bounded retry reply):
/// identical span-clamp handling to `handle_highlighted`'s `Some` case via
/// the shared `apply_highlight_spans`, but a SECOND `None` for a
/// still-never-highlighted document surfaces a status line instead of
/// retrying again — this function never calls `schedule_highlight`/
/// `highlight::retry_highlight` from that arm, so it is provably the last
/// attempt for this failed first-highlight chain; a later edit still
/// schedules a fresh one normally, starting the same one-retry chain over.
fn handle_highlight_retried(
    app: &mut App,
    id: DocumentId,
    version: u64,
    result: Option<rune_ts::HighlightResult>,
    effects: &mut Effects,
) {
    let mut exhausted = false;
    let mut pending = false;
    if let Some(doc) = app.doc_mut(id) {
        doc.highlight.in_flight = None;
        pending = doc.highlight.pending;
        doc.highlight.pending = false;
        let live_version = doc.buffer.version();
        match result {
            Some(spans) if version == live_version => {
                doc.highlight.truncated = spans.truncated;
                apply_highlight_spans(doc, version, spans.spans);
            }
            None if version == live_version && doc.highlight.version == 0 => {
                exhausted = true;
            }
            _ => {}
        }
    }

    if exhausted {
        app.set_status(
            "syntax highlighting timed out for this document",
            crate::app::StatusSource::Other,
        );
    }

    if pending {
        crate::highlight::schedule_highlight(app, id, effects);
    }
}

/// The four-stage key pipeline (plan Context, decision 8): (1) modal capture
/// (`banner::handle_key`) — every key consumed there while `App.modal` is
/// `Some`, quit chords included; (2) the global chord table
/// (`GLOBAL_BINDINGS`), fired regardless of focus (WP2.S4); (3) the focused
/// pane's own keymap; (4) `Ignored` -> nothing.
pub(crate) fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    // Take and clear the held-space arming FIRST (plan WP5.S5): whatever
    // this keystroke turns out to be, the arming from the space typed
    // immediately before it must never survive past this one lookup.
    let speculative_space = app.speculative_space.take();

    // Stage 1: modal capture, before any other stage ever sees this key.
    if app.modal.is_some() {
        crate::banner::handle_key(app, key, effects);
        return;
    }

    // Stage 1.5: held-space leader completion (plan WP5.S6, decision 2) —
    // after modal capture (a modal owns the keyboard; space under a modal
    // stays a consumed no-op) and before the global table (§3.4: "chord
    // completions -> global actions"). Confirms the physical key state only
    // once an `x`/`e`/`t` press has already arrived, never standalone.
    if let Some(cmd) = crate::binding::resolve_in(crate::global::LEADER_BINDINGS, key)
        && app.space_probe.space_is_down()
    {
        if let Some(doc) = speculative_space {
            edit::retract_space(app, doc);
        }
        pane::handle_global_command(app, cmd, effects);
        return;
    }

    // Stage 2: global chrome keys, before any pane sees the key.
    if let Some(cmd) = keymap::resolve_in(keymap::GLOBAL_BINDINGS, key) {
        pane::handle_global_command(app, cmd, effects);
        return;
    }

    // Stage 3 + stage 4 (no further stage yet, so the `Ignored` outcome is
    // captured but unused).
    let _outcome = match app.focus {
        Pane::Editor => handle_editor_key(app, key, effects),
        Pane::Explorer => explorer::handle_key(app, key, effects),
        Pane::Tabs => opentabs::handle_key(app, key),
        Pane::Title => crate::title::handle_key(app, key, effects),
    };
}

/// The editor pane's own key handling — the pre-WP2 `handle_key` body,
/// reached only when `app.focus == Pane::Editor`. `Save`/`QuitConfirm` stay
/// reachable here too, though stage 2 above always intercepts those first.
fn handle_editor_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> keymap::KeyOutcome {
    // Hardcoded fast paths outside the resolver, exactly as Go
    // (`textedit/update.go:67-85`): Enter (mod 0) -> newline; Escape ->
    // collapse selection. Neither is a resolver-bound chord (plan Context,
    // "Keymap").
    if key.code == KeyCode::Enter && key.mods == Mods::NONE {
        edit::newline(app, app.active);
        return keymap::KeyOutcome::Consumed;
    }
    if key.code == KeyCode::Escape && key.mods == Mods::NONE {
        nav::escape(app.active_doc_mut());
        return keymap::KeyOutcome::Consumed;
    }

    let Some(command) = keymap::resolve(key) else {
        // Unmatched printable text -> insert fallthrough (plan Context,
        // "Hardcoded fast paths outside the resolver": `update.go:134-158`).
        // Ctrl/Alt/Super chords that reach here are simply unbound, never an
        // insert — every bound Ctrl/Alt/Super chord is already caught by
        // `keymap::resolve` above.
        if let KeyCode::Char(ch) = key.code
            && !key.mods.ctrl
            && !key.mods.alt
            && !key.mods.sup
            && is_insertable_key_char(ch)
        {
            edit::insert_char(app, app.active, ch);
            // Arms the held-space leader (plan WP5.S7/decision 2/A3): the
            // ONLY arming site — a directly-typed space types immediately,
            // with zero latency, and a following space-held `x`/`e`/`t`
            // retracts it. Paste, indent-carrying newline and redo insert
            // spaces too but deliberately do not arm this (assumption A3).
            if ch == ' ' {
                app.speculative_space = Some(app.active);
            }
            return keymap::KeyOutcome::Consumed;
        }
        return keymap::KeyOutcome::Ignored;
    };

    match command {
        Command::CharLeft => nav::char_left(app.active_doc_mut(), false),
        Command::CharRight => nav::char_right(app.active_doc_mut(), false),
        // Up at the very top of the buffer focuses the title instead — a
        // contextual gesture, not a new binding, so §3.1's one-key-one-
        // binding rule is untouched. Anywhere else it's an ordinary
        // cursor move.
        Command::LineUp => {
            if at_buffer_top(app) {
                pane::focus_title(app);
            } else {
                nav_scroll::line_up(app.active_doc_mut(), false);
            }
        }
        Command::LineDown => nav_scroll::line_down(app.active_doc_mut(), false),
        Command::WordLeft => nav::word_left(app.active_doc_mut(), false),
        Command::WordRight => nav::word_right(app.active_doc_mut(), false),
        Command::LineStart => nav::line_start(app.active_doc_mut(), false),
        Command::LineEnd => nav::line_end(app.active_doc_mut(), false),
        Command::PageUp => nav_scroll::page_up(app.active_doc_mut(), false),
        Command::PageDown => nav_scroll::page_down(app.active_doc_mut(), false),
        Command::SelectCharLeft => nav::char_left(app.active_doc_mut(), true),
        Command::SelectCharRight => nav::char_right(app.active_doc_mut(), true),
        Command::SelectLineUp => nav_scroll::line_up(app.active_doc_mut(), true),
        Command::SelectLineDown => nav_scroll::line_down(app.active_doc_mut(), true),
        Command::SelectWordLeft => nav::word_left(app.active_doc_mut(), true),
        Command::SelectWordRight => nav::word_right(app.active_doc_mut(), true),
        Command::SelectLineStart => nav::line_start(app.active_doc_mut(), true),
        Command::SelectLineEnd => nav::line_end(app.active_doc_mut(), true),
        Command::SelectPageUp => nav_scroll::page_up(app.active_doc_mut(), true),
        Command::SelectPageDown => nav_scroll::page_down(app.active_doc_mut(), true),
        Command::SelectAll => nav::select_all(app.active_doc_mut()),
        Command::ScrollLineUp => nav_scroll::scroll_line_up(app.active_doc_mut()),
        Command::ScrollLineDown => nav_scroll::scroll_line_down(app.active_doc_mut()),
        Command::ScrollHalfPageUp => nav_scroll::scroll_half_page_up(app.active_doc_mut()),
        Command::ScrollHalfPageDown => nav_scroll::scroll_half_page_down(app.active_doc_mut()),
        Command::CentreCursor => nav_scroll::centre_cursor(app.active_doc_mut()),
        Command::CursorToTop => nav_scroll::cursor_to_top(app.active_doc_mut()),
        Command::CursorToBottom => nav_scroll::cursor_to_bottom(app.active_doc_mut()),
        Command::DeleteLeft => edit::delete_left(app, app.active),
        Command::DeleteRight => edit::delete_right(app, app.active),
        Command::DeleteWordLeft => edit::delete_word_left(app, app.active),
        Command::DeleteWordRight => edit::delete_word_right(app, app.active),
        Command::DeleteLine => edit_lines::delete_line(app, app.active),
        Command::Indent => edit_lines::indent(app, app.active),
        Command::Outdent => edit_lines::outdent(app, app.active),
        Command::MoveLineUp => edit_lines::move_line_up(app, app.active),
        Command::MoveLineDown => edit_lines::move_line_down(app, app.active),
        Command::CloneLineUp => edit_lines::clone_line_up(app, app.active),
        Command::CloneLineDown => edit_lines::clone_line_down(app, app.active),
        Command::AddCursorAbove => multi::add_cursor_above(app.active_doc_mut()),
        Command::AddCursorBelow => multi::add_cursor_below(app.active_doc_mut()),
        Command::Undo => edit::undo(app, app.active),
        Command::Redo => edit::redo(app, app.active),
        Command::Copy => clipboard::copy(app, app.active, effects),
        Command::Cut => clipboard::cut(app, app.active, effects),
        Command::Paste => clipboard::paste(effects),
        Command::Save => save::trigger_save(app, app.active, effects),
        Command::QuitConfirm => {
            // `resolve` only ever returns `QuitConfirm` when `key` is a
            // known quit chord (see `keymap::QuitKey::from_key`, the single
            // source of truth both functions route through). Dead in
            // practice — stage 2 (`keymap::GLOBAL_BINDINGS`) always
            // intercepts a quit chord before it reaches here.
            if let Some(quit_key) = QuitKey::from_key(key) {
                pane::handle_quit_key(app, quit_key, effects);
            }
        }
    }
    keymap::KeyOutcome::Consumed
}

/// Guards the printable-insert fallthrough against control-byte leakage
/// (data-integrity fix, review finding F1). Go's equivalent gate is
/// `isPrintableChar` (`textedit.go:441-443`: `r >= ' ' && r <= '~'`), but
/// that gate applies ONLY to Go's SYNTHESIZED-from-`BaseCode` case
/// (`update.go:136-145`) — real decoded text (`msg.Text`, and everything
/// `Msg::Paste` carries here) flows unrestricted, including non-ASCII
/// (CJK, emoji). This crate's termina-backed `KeyCode::Char(char)` has no
/// such split: it is Go's `BaseCode` concept alone, never a separate
/// decoded-text stream, so a literal ASCII-only port would also block
/// genuine direct-keystroke Unicode entry Go itself allows unrestricted
/// (and which `tests/tui_edit.rs` requires). The hazard Go's gate actually
/// closes is narrower than "ASCII only": a raw C0 control byte or DEL
/// leaking through as `Char` with no modifier flag at all — the reported
/// case is a non-Kitty terminal's legacy encoding, where Ctrl+A IS the
/// literal SOH byte (no separate "this was a chord" signal survives
/// decoding) rather than a Kitty-protocol key report with an explicit
/// Ctrl modifier. Such a leaked byte can only ever be a single codepoint
/// in `0x00..=0x1F` or `0x7F` — ASCII's own control range — so excluding
/// `char::is_control()` (Unicode category Cc: `0x00..=0x1F` and
/// `0x7F..=0x9F`) closes that exact hazard without narrowing what a human
/// can actually type.
/// Whether the active document's primary cursor sits on the FIRST display
/// line — `display_position` is one-indexed, so line 1 is the top.
fn at_buffer_top(app: &App) -> bool {
    let doc = app.active_doc();
    let offset = doc.cursors.primary().position;
    doc.buffer.display_position(offset).0 == 1
}

fn is_insertable_key_char(ch: char) -> bool {
    !ch.is_control()
}
