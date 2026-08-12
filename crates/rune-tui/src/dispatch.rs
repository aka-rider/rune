//! The `Msg` dispatcher, key pipeline, and `rune-db` event router, split out
//! of `app` (the 500-line budget): `update_inner` is the top-level `Msg` match
//! `app::update` used to run inline; its `Msg::Key`/`Msg::Db` arms route to
//! the four-stage key pipeline and the `rune-db` ack router right below.
//! Nothing else changes — every one of these is exactly the function
//! `app.rs` used to define locally, now reached through `dispatch::`
//! instead.

use crate::app::App;
use crate::commands::{
    clipboard, edit, edit_lines, edit_lines_move, mouse, multi, nav, nav_line, nav_scroll,
    reading_nav,
};
use crate::document::DocumentId;
use crate::highlight::HighlightReply;
use crate::keymap::{self, Command, Extend, KeyCode, KeyInput, Mods, Motion, QuitKey};
use crate::navigate;
use crate::pane::{self, Pane};
use crate::runtime::{Effects, Msg, PasteTarget};
use crate::{explorer, explorer_keys, materialize_ack, opentabs, save};

/// The one dispatcher every `Msg` funnels through (`app::update`'s inner
/// half, split out here alongside the key/db-event routers it calls into —
/// the 500-line budget). `app::update` wraps this with the snapshot-autosave
/// debounce chokepoint; nothing else in the crate calls this directly.
pub(crate) fn update_inner(app: &mut App, msg: Msg, effects: &mut Effects) {
    match msg {
        Msg::Key(key) => handle_key(app, key, effects),
        Msg::Mouse(input) => mouse::handle(app, input, effects),
        Msg::Resize(width, height) => {
            app.frame_width = width;
            app.frame_height = height;
            app.relayout();
            // A settled focus is a claim about what THIS frame paints, and
            // this write is the one place that claim can go stale without
            // any focus transition ever running: `LayoutMode::resolve` reads
            // `frame_width`/`frame_height` alone, so the mode can flip while
            // `app.focus` never moved. `reconcile` already repairs this same
            // shape for the collapse command and the splitter drag; this is
            // the resize path's counterpart. Called after `relayout` so the
            // dimensions `layout_mode` reads back are the ones just written
            // above, not the previous frame's.
            crate::focus::reconcile(app, effects);
            // The pane may have just changed width (or the
            // terminal's reported cell pixel geometry may — see
            // `refit_on_resize`'s own docs), so a `Live` image document's
            // fit-to-width footprint can need re-fitting and retransmitting.
            crate::graphics::refit_on_resize(app, effects);
        }
        Msg::Paste(text) => clipboard::route_bracketed_paste(app, &text, effects),
        Msg::ClipboardRead { text, target } => {
            // Same deliberate no-modal-gate rule as bracketed paste above.
            match target {
                PasteTarget::Title(doc) => crate::title::keys::paste(app, doc, &text),
                PasteTarget::Document(id) => clipboard::handle_paste_content(app, id, &text),
                PasteTarget::Search => crate::search::keys::paste(app, &text),
            }
        }
        Msg::SaveDone {
            id,
            ticket,
            version,
            result,
            durable,
        } => materialize_ack::handle_save_done(app, id, ticket, version, result, durable),
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
        // A stale generation (a newer post/focus/collapse superseded this
        // one since it was armed) is ignored, mirroring `ConfirmTimeout`/
        // `SaveConfirmTimeout` above.
        Msg::MessagesCollapseTimeout { generation } => {
            if crate::messages::is_armed(app, generation) {
                crate::messages::collapse(app);
                crate::focus::reconcile(app, effects);
            }
        }
        Msg::SnapshotDue { id, generation } => {
            materialize_ack::handle_snapshot_due(app, id, generation)
        }
        Msg::Db(evt) => crate::db_dispatch::handle_db_event(app, evt, effects),
        Msg::MaterializeVfsDone {
            id,
            ticket,
            db_id,
            seq,
            content,
            outcome,
        } => materialize_ack::handle_materialize_vfs_done(
            app, id, ticket, db_id, seq, content, outcome,
        ),
        Msg::DirLoaded {
            root,
            entries,
            cause,
            generation,
        } => explorer::handle_dir_loaded(app, root, entries, cause, generation),
        Msg::RenameDone { generation, result } => {
            crate::rename::handle_rename_done(app, generation, result, effects)
        }
        Msg::TrashDone {
            generation,
            path,
            result,
        } => crate::trash::handle_trash_done(app, generation, path, result, effects),
        Msg::FileOpened {
            path,
            result,
            anchor,
        } => crate::workspace::handle_file_opened(app, path, result, anchor, effects),
        Msg::Highlighted {
            doc,
            version,
            result,
        } => handle_highlighted(app, doc, version, result, effects),
        Msg::BootstrapViewReady {
            id,
            version,
            machine,
            view,
        } => handle_bootstrap_view_ready(app, id, version, machine, view),
        Msg::ImageDecoded {
            doc,
            generation,
            result,
        } => crate::graphics::handle_image_decoded(app, doc, generation, result, effects),
        Msg::EmbedDecoded {
            doc,
            generation,
            result,
        } => crate::graphics::handle_embed_decoded(app, doc, generation, result, effects),
        Msg::Error(e) => crate::messages::error(app, e),
        Msg::Warning(w) => crate::messages::warn(app, w),
        Msg::SearchHistory { generation, result } => {
            crate::search::handle_history_loaded(app, generation, result)
        }
        Msg::FileSearchRecentsLoaded { generation, result } => {
            crate::filesearch::handle_recents_loaded(app, generation, result, effects)
        }
        Msg::FileSearchScanned { generation, result } => {
            crate::filesearch::handle_scanned(app, generation, result, effects)
        }
        Msg::Quit => {
            app.should_quit = true;
        }
    }
}

// `handle_db_event` (the `rune-db` ack router) moved to `db_dispatch.rs`
// (500-line budget) — `update_inner`'s `Msg::Db` arm above calls it through
// `db_dispatch::`.

/// The post-dispatch chokepoint `app::update` calls after every message
/// (moved out of `app.rs` itself, which already exceeds the 500-line ceiling
/// from unrelated concurrent work and must not grow further): whatever a
/// content/cursor/tab-switch change can invalidate that `update_inner`
/// didn't already handle inline. Highlight scheduling and a newly-active
/// image DOCUMENT's decode are unchanged; `sync_embeds` runs
/// unconditionally (its own `app.graphics.kitty`/`doc.kind` guards make it
/// a cheap no-op the overwhelming majority of the time) so no future edit
/// path can forget to keep the active document's embed set current.
pub(crate) fn after_update(
    app: &mut App,
    active_before: DocumentId,
    buffer_version_before: u64,
    effects: &mut Effects,
) {
    if app.active != active_before || app.active_doc().buffer.version() != buffer_version_before {
        let id = app.active;
        crate::highlight::schedule_highlight(app, id, effects);
    }
    if app.active != active_before {
        crate::graphics::schedule_image_decode(app, app.active, effects);
    }
    crate::graphics::sync_embeds(app, app.active, effects);
    // The message pane's auto-collapse arming is re-evaluated after every
    // settle rather than only right after a post, so the
    // countdown starts (or restays suppressed) correctly no matter what
    // changed focus/selection/log state in between — the four suppression
    // rules live in `should_arm_auto_collapse` itself.
    if crate::messages::should_arm_auto_collapse(app) {
        let generation = crate::messages::arm_auto_collapse(app);
        effects
            .cmds
            .push(crate::messages::collapse_timeout_cmd(generation));
    }
}

/// Applies a `Msg::BootstrapViewReady` reply: a plain wholesale swap when
/// the buffer hasn't moved on since the compute was dispatched (the
/// ordinary bootstrap case — nothing else reaches `update` before this
/// reply lands), dropped otherwise. A dropped reply is not a hazard: the
/// next `App::sync_view` the main loop already runs after every message
/// recomputes from the live, edited buffer through the ordinary synchronous
/// path — the same one `bootstrap`'s large-document branch deferred, now
/// fast per issue #11's own fix.
fn handle_bootstrap_view_ready(
    app: &mut App,
    id: DocumentId,
    version: u64,
    machine: Box<rune_md::element::doc::DocMachine>,
    view: rune_md::element::doc::ViewSnapshots,
) {
    let Some(doc) = app.doc_mut(id) else { return };
    if doc.buffer.version() != version {
        return;
    }
    doc.doc = *machine;
    doc.view = Some(view);
}

/// Applies a `Msg::Highlighted` reply, in the fixed order `[R2]` requires:
/// (a) `in_flight` clears regardless of what the reply carries, so a
/// document can never deadlock waiting on a highlight that already returned;
/// (b) `result: None` (every attempted region overran its budget, or none
/// resolved) leaves every region exactly as it was — a slow document
/// degrades to STALE colours, never to none; (c) a `version` that no longer
/// matches the live buffer means a NEWER edit landed while this reply was in
/// flight, so it describes stale content and is dropped whole, regions again
/// left untouched; (d) otherwise the reply's whole region layout is
/// installed; (e) if a further edit arrived while this reply was in flight
/// (`pending`), it is cleared and a fresh highlight is requested immediately
/// rather than waiting for the next keystroke.
///
/// A `None` reply at the live version surfaces a status line — for a fence
/// exactly as for a whole file, since both now run the same single bounded
/// parse per region and both fail the same way. It is narrowed to a document
/// that has never once been successfully highlighted (`highlight.version ==
/// 0`, which `Buffer::version` itself never produces): once a document has
/// colours, a reparse-after-edit that overruns the budget degrades to STALE
/// colours per `[R2]` and stays silent instead of spamming the status on
/// every settled edit of a large file.
///
/// A terminal timeout must never re-dispatch a further parse for a no-edit
/// `pending`: an edit-armed `pending` carries a version that differs from
/// the reply's, landing in the stale arm below, so `pending` and `timed_out`
/// can coincide only when nothing but a document switch armed `pending` — in
/// that case re-scheduling would just repeat the same doomed parse.
fn handle_highlighted(
    app: &mut App,
    id: DocumentId,
    version: u64,
    result: Option<HighlightReply>,
    effects: &mut Effects,
) {
    let mut timed_out = false;
    let mut pending = false;
    let mut truncated = false;
    if let Some(doc) = app.doc_mut(id) {
        doc.highlight.in_flight = None;
        pending = doc.highlight.pending;
        doc.highlight.pending = false;
        let live_version = doc.buffer.version();
        match result {
            Some(reply) if version == live_version => {
                crate::highlight::apply_reply(doc, version, reply);
            }
            None if version == live_version && doc.highlight.version == 0 => {
                timed_out = true;
            }
            // A `None` reply for a document that already has colours, or any
            // reply at a stale version, leaves every region untouched —
            // `[R2]`.
            _ => {}
        }
        truncated = doc.highlight.truncated;
    }

    // Timeout and truncation can coincide (truncation is sticky state from
    // an earlier reply, timeout is decided fresh above) — only one status
    // line is ever shown per reply, and timeout wins: it means NOTHING
    // coloured this round, which is more actionable than a coloured-but-
    // incomplete tail.
    if timed_out {
        crate::messages::warn(app, "syntax highlighting timed out for this document");
    } else if truncated {
        crate::messages::warn(
            app,
            "syntax highlighting was truncated; the tail of this document is uncoloured",
        );
    }

    if pending && !timed_out {
        crate::highlight::schedule_highlight(app, id, effects);
    }
}

/// The four-stage key pipeline: (1) Guard capture (`guard::handle_guard_key`)
/// — every key consumed there while `App.guard` is `Some`, quit chords
/// included; the old modal error banner no longer exists, so this stage is
/// Guard-only now — the messages pane is non-modal and reached through
/// stage 3 instead, like any other pane; (2) the global chord table
/// (`GLOBAL_BINDINGS`), fired regardless of focus; (3) the focused pane's
/// own keymap; (4) `Ignored` -> nothing.
pub(crate) fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    // Stage 1: Guard capture, before any other stage ever sees this key.
    if app.guard.is_some() {
        crate::guard::handle_guard_key(app, key, effects);
        return;
    }

    // Stage 2: global chrome keys, before any pane sees the key.
    if let Some(cmd) = keymap::resolve_in(keymap::GLOBAL_BINDINGS, key) {
        pane::handle_global_command(app, cmd, effects);
        return;
    }

    // Stage 3, search-bar branch: checked BEFORE the chrome-level `Pane`
    // match below, since the bar is its own focus state, never a `Pane`
    // variant (`focus::target`'s own "second input checked first" doc).
    if crate::focus::target(app) == crate::focus::FocusTarget::SearchField {
        let _ = crate::search::keys::handle_key(app, key, effects);
        return;
    }

    // Stage 3, file-finder branch: same shape as the search-bar branch
    // above, checked before the chrome-level `Pane` match — the finder is
    // never a `Pane` either.
    if crate::focus::target(app) == crate::focus::FocusTarget::FileSearch {
        let _ = crate::filesearch::keys::handle_key(app, key, effects);
        return;
    }

    // Stage 3 + stage 4: the focused pane's own keymap. There is no stage
    // 5 to react to `KeyOutcome::Ignored` with, so the verdict is discarded
    // here rather than threaded anywhere further.
    let _ = match app.focus() {
        Pane::Editor => handle_editor_key(app, key, effects),
        Pane::Explorer => explorer_keys::handle_key(app, key, effects),
        Pane::Tabs => opentabs::handle_key(app, key, effects),
        Pane::Title => crate::title::handle_key(app, key, effects),
        Pane::Messages => {
            if crate::messages::handle_key(app, key, effects) {
                keymap::KeyOutcome::Consumed
            } else {
                keymap::KeyOutcome::Ignored
            }
        }
    };
}

/// The editor pane's own key handling,
/// reached only when `app.focus() == Pane::Editor`. `Save`/`QuitConfirm` stay
/// reachable here too, though stage 2 above always intercepts those first.
fn handle_editor_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> keymap::KeyOutcome {
    // The merge resolver owns EVERY key while active on this document —
    // checked before the hardcoded Enter/Escape fast paths and the
    // printable-insert fallthrough below, or `o`/`t`/`b` would type into
    // the working form and bare Esc would collapse the selection instead
    // of closing the merge.
    if crate::merge::keys::intercept(app, key) {
        return keymap::KeyOutcome::Consumed;
    }

    // Hardcoded fast paths outside the resolver: Enter (mod 0) -> newline;
    // Escape -> collapse selection. Neither is a resolver-bound chord (plan
    // Context, "Keymap").
    if key.code == KeyCode::Enter && key.mods == Mods::NONE {
        if app.active_doc().is_read_only() {
            navigate::follow(app, effects);
        } else {
            edit::newline(app, app.active);
        }
        return keymap::KeyOutcome::Consumed;
    }
    if key.code == KeyCode::Escape && key.mods == Mods::NONE {
        // The cascade — multi-cursor, then selection, then leave to the
        // Explorer: `nav::escape` collapses whichever of the first two it
        // finds and reports `Unconsumed` only once neither applies, which
        // is this fast path's own cue to hand focus to the Explorer instead
        // — unfolding the left column if it's collapsed, and landing the
        // cursor on the active document's file (`pane::
        // show_and_focus_explorer_on_active_file`, shared with `^B`'s show
        // branch).
        if nav::escape(app.active_doc_mut()) == nav::EscapeOutcome::Unconsumed {
            pane::show_and_focus_explorer_on_active_file(app, effects);
        }
        return keymap::KeyOutcome::Consumed;
    }

    let Some(command) = keymap::resolve(key) else {
        // Unmatched printable text -> insert fallthrough.
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
            return keymap::KeyOutcome::Consumed;
        }
        return keymap::KeyOutcome::Ignored;
    };

    // A read-only document has no insertion point, so every motion key is a
    // viewport command instead (`commands::reading_nav`'s module docs) —
    // checked before the big match below so none of its cursor-driven
    // handlers ever runs against a document that paints no caret to show
    // the cursor moving.
    if reading_nav::intercept(app, command) {
        return keymap::KeyOutcome::Consumed;
    }

    match command {
        Command::Motion(Motion::CharLeft, extend) => nav::char_left(app.active_doc_mut(), extend),
        Command::Motion(Motion::CharRight, extend) => nav::char_right(app.active_doc_mut(), extend),
        // Up at the very top of the buffer focuses the title instead — a
        // contextual gesture, not a new binding, so the one-key-one-
        // binding rule is untouched. Anywhere else it's an ordinary
        // cursor move. A read-only document never reaches this arm:
        // `reading_nav::intercept` above re-keys the same gesture to the
        // view's own top and consumes the key first.
        Command::Motion(Motion::LineUp, Extend::No) => {
            if at_buffer_top(app) {
                app.focus_title();
            } else {
                nav_scroll::line_up(app.active_doc_mut(), Extend::No);
            }
        }
        Command::Motion(Motion::LineUp, Extend::Yes) => {
            nav_scroll::line_up(app.active_doc_mut(), Extend::Yes);
        }
        Command::Motion(Motion::LineDown, extend) => {
            nav_scroll::line_down(app.active_doc_mut(), extend)
        }
        Command::Motion(Motion::WordLeft, extend) => nav::word_left(app.active_doc_mut(), extend),
        Command::Motion(Motion::WordRight, extend) => nav::word_right(app.active_doc_mut(), extend),
        Command::Motion(Motion::LineStart, extend) => {
            nav_line::line_start(app.active_doc_mut(), extend)
        }
        Command::Motion(Motion::LineEnd, extend) => {
            nav_line::line_end(app.active_doc_mut(), extend)
        }
        Command::Motion(Motion::PageUp, extend) => {
            nav_scroll::page_up(app.active_doc_mut(), extend)
        }
        Command::Motion(Motion::PageDown, extend) => {
            nav_scroll::page_down(app.active_doc_mut(), extend)
        }
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
        Command::MoveLineUp => edit_lines_move::move_line_up(app, app.active),
        Command::MoveLineDown => edit_lines_move::move_line_down(app, app.active),
        Command::CloneLineUp => edit_lines_move::clone_line_up(app, app.active),
        Command::CloneLineDown => edit_lines_move::clone_line_down(app, app.active),
        Command::AddCursorAbove => multi::add_cursor_above(app.active_doc_mut()),
        Command::AddCursorBelow => multi::add_cursor_below(app.active_doc_mut()),
        Command::Undo => edit::undo(app, app.active),
        Command::Redo => edit::redo(app, app.active),
        Command::Copy => clipboard::copy(app, app.active, effects),
        Command::Cut => clipboard::cut(app, app.active, effects),
        Command::Paste => clipboard::paste(effects, PasteTarget::Document(app.active)),
        Command::Save => {
            let _ = save::trigger_save(app, app.active, save::SaveMode::Normal, effects);
        }
        Command::FollowLink => navigate::follow(app, effects),
        Command::Reload => {
            if app.active_doc().has_reloadable_graphics() {
                crate::graphics::reload_image(app, app.active, effects);
                crate::graphics::reload_embeds(app, app.active, effects);
            } else {
                crate::messages::info(app, "nothing to reload");
            }
        }
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
/// (data-integrity fix, review finding F1). An ASCII-only filter (`' '..=
/// '~'`) would be wrong here: this crate's termina-backed
/// `KeyCode::Char(char)` carries both synthesized single-key chords and
/// genuine decoded text (`msg.Text`, and everything `Msg::Paste` carries
/// here) with no way to tell them apart, so restricting to that range would
/// also block real non-ASCII keystrokes — CJK, emoji — that
/// `tests/tui_edit.rs` requires to flow unrestricted. The hazard actually
/// worth closing is narrower than "ASCII only": a raw C0 control byte or DEL
/// leaking through as `Char` with no modifier flag at all — the reported
/// case is a non-Kitty terminal's legacy encoding, where Ctrl+A IS the
/// literal SOH byte (no separate "this was a chord" signal survives
/// decoding) rather than a Kitty-protocol key report with an explicit
/// Ctrl modifier. Such a leaked byte can only ever be a single codepoint
/// in `0x00..=0x1F` or `0x7F` — ASCII's own control range — so excluding
/// `char::is_control()` (Unicode category Cc: `0x00..=0x1F` and
/// `0x7F..=0x9F`) closes that exact hazard without narrowing what a human
/// can actually type.
/// Whether the active document's primary cursor sits on the FIRST buffer
/// line — `offset_to_line_col`'s `line` is 0-indexed, so line 0 is the top.
/// Only ever asked about an editable document: a read-only one has no
/// insertion point to test, and `commands::reading_nav` answers the same
/// question there against the viewport's own top instead.
fn at_buffer_top(app: &App) -> bool {
    let doc = app.active_doc();
    let offset = doc.cursors.primary().position;
    doc.buffer.offset_to_line_col(offset).line == 0
}

fn is_insertable_key_char(ch: char) -> bool {
    !ch.is_control()
}
