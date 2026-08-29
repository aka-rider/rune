use crate::app::App;
use crate::commands::{clipboard, edit, editor_exec, mouse};
use crate::document::DocumentId;
use crate::focus::FocusTarget;
use crate::highlight::PassOutcome;
use crate::keymap::{self, KeyCode, KeyInput, Mods, QuitKey};
use crate::pane;
use crate::runtime::{Effects, Msg, PasteTarget, TimerKey, TimerMsgKey};
use crate::{explorer, explorer_keys, materialize_ack, opentabs};

pub(crate) fn update_inner(app: &mut App, msg: Msg, effects: &mut Effects) {
    match msg {
        Msg::Key(key) => handle_key(app, key, effects),
        Msg::PumpGraphics => {}
        Msg::Mouse(input) => mouse::handle(app, input, effects),
        Msg::Resize(width, height) => {
            app.frame = Some(crate::app::FrameSize::new(width, height));
            app.relayout();
            crate::focus::reconcile(app, effects);
            crate::graphics::refit_on_resize(app, effects);
        }
        Msg::Paste(text) => clipboard::route_bracketed_paste(app, &text, effects),
        Msg::ClipboardRead { text, target } => match target {
            PasteTarget::Title(doc) => crate::title::keys::paste(app, doc, &text),
            PasteTarget::Document(id) => clipboard::handle_paste_content(app, id, &text),
            PasteTarget::Search => crate::search::keys::paste(app, &text),
            PasteTarget::Palette => crate::palette::keys::paste(app, &text),
        },
        Msg::SaveDone {
            id,
            ticket,
            version,
            result,
            detail,
        } => materialize_ack::handle_save_done(app, id, ticket, version, result, detail),
        Msg::Timer { key, generation } => match key {
            TimerMsgKey::QuitConfirm => {
                let generation = crate::generation::QuitGen::from_raw(generation);
                if let crate::app::QuitNegotiation::ConfirmArmed(_, pending_gen) = app.quit
                    && pending_gen == generation
                {
                    app.quit = crate::app::QuitNegotiation::Idle;
                }
            }
            TimerMsgKey::SaveConfirm => {
                let generation = crate::generation::SaveConfirmGen::from_raw(generation);
                if app
                    .pending_save_confirm
                    .is_some_and(|(_, g)| g == generation)
                {
                    app.pending_save_confirm = None;
                }
            }
            TimerMsgKey::MessagesCollapse => {
                let generation = crate::generation::MessagesCollapseGen::from_raw(generation);
                if crate::messages::is_armed(app, generation) {
                    crate::messages::collapse(app);
                    crate::focus::reconcile(app, effects);
                }
            }
        },
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
            app, id, ticket, db_id, seq, &content, outcome,
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
        } => crate::trash::handle_trash_done(app, generation, &path, result, effects),
        Msg::FileOpened {
            path,
            result,
            anchor,
            preview_generation,
        } => crate::workspace::handle_file_opened(
            app,
            &path,
            result,
            anchor,
            preview_generation,
            effects,
        ),
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
        Msg::ImageEncoded {
            doc,
            generation,
            was_live,
            result,
        } => crate::graphics::handle_image_encoded(app, doc, generation, was_live, result, effects),
        Msg::EmbedEncoded {
            doc,
            generation,
            result,
        } => crate::graphics::handle_embed_encoded(app, doc, generation, result, effects),
        Msg::Posted { severity, text } => crate::messages::post(app, severity, text),
        Msg::RecentsLoaded { generation, result } => match result {
            crate::runtime::RecentsResult::Search(result) => crate::search::handle_history_loaded(
                app,
                crate::generation::SearchHistoryGen::from_raw(generation),
                result,
            ),
            crate::runtime::RecentsResult::FileSearch(result) => {
                crate::filesearch::handle_recents_loaded(
                    app,
                    crate::generation::FileSearchGen::from_raw(generation),
                    result,
                    effects,
                )
            }
            crate::runtime::RecentsResult::Palette(result) => {
                crate::palette::handle_recents_loaded(
                    app,
                    crate::generation::PaletteGen::from_raw(generation),
                    result,
                )
            }
        },
        Msg::FileSearchScanned { generation, result } => {
            crate::filesearch::handle_scanned(app, generation, result, effects)
        }
        Msg::KeyboardFlagsReport(flags) => handle_keyboard_flags_report(app, flags),
        Msg::Quit => {
            app.should_quit = true;
        }
    }
}

/// The terminal's reply to the Kitty keyboard-protocol flags query
/// `term::Guard::enter_app_mode` sends on startup. Settles
/// `app.keyboard_flags` from `None` to a real answer exactly once per
/// session, and posts a one-time note when that answer confirms the
/// terminal dropped a bit this app asked for, since a `⌘` chord built on a
/// dropped bit is a guess no dropped chord should stay silent about.
fn handle_keyboard_flags_report(app: &mut App, flags: termina::escape::csi::KittyKeyboardFlags) {
    app.keyboard_flags = Some(flags);
    if !crate::term::sup_chords_reliable(app.keyboard_flags) {
        crate::messages::warn_if_new(
            app,
            "this terminal never confirmed key disambiguation \u{2014} \u{2318} chords marked \u{26a0} in Help (F1) may not reach rune here",
        );
    }
}

pub(crate) fn after_update(
    app: &mut App,
    active_before: DocumentId,
    buffer_version_before: u64,
    frame_width_before: u16,
    effects: &mut Effects,
) {
    let content_changed =
        app.active != active_before || app.active_doc().buffer.version() != buffer_version_before;
    if content_changed {
        let id = app.active;
        crate::highlight::schedule_highlight(app, id, effects);
    }
    if app.active != active_before {
        crate::graphics::schedule_image_decode(app, app.active, effects);
    }
    if content_changed || app.frame_width() != frame_width_before {
        crate::graphics::sync_embeds(app, app.active, effects);
    }
    if crate::messages::should_arm_auto_collapse(app) {
        let generation = crate::messages::arm_auto_collapse(app);
        app.timers.arm(
            TimerKey::from(TimerMsgKey::MessagesCollapse),
            crate::messages::AUTO_COLLAPSE,
            Msg::Timer {
                key: TimerMsgKey::MessagesCollapse,
                generation: generation.raw(),
            },
        );
    }
    if app.palette().is_some() {
        crate::palette::sync_stale(app);
    }
}

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

fn handle_highlighted(
    app: &mut App,
    id: DocumentId,
    version: u64,
    result: PassOutcome,
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
            PassOutcome::Replace(reply) if version == live_version => {
                crate::highlight::apply_reply(doc, version, reply);
            }
            PassOutcome::CarryForward if version == live_version && doc.highlight.version == 0 => {
                timed_out = true;
            }
            _ => {}
        }
        truncated = doc.highlight.truncated;
    }

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

pub(crate) fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    if app.guard.is_some() {
        crate::guard::handle_guard_key(app, key, effects);
        return;
    }

    if let Some(cmd) = keymap::resolve_in(keymap::GLOBAL_BINDINGS, key) {
        pane::handle_global_command(app, cmd, effects);
        return;
    }

    let _ = match crate::focus::target(app) {
        FocusTarget::SearchField | FocusTarget::ReplaceField => {
            crate::search::keys::handle_key(app, key, effects)
        }
        FocusTarget::FileSearch => crate::filesearch::keys::handle_key(app, key, effects),
        FocusTarget::ProjectSearch => crate::projectsearch::keys::handle_key(app, key, effects),
        FocusTarget::Palette => crate::palette::keys::handle_key(app, key, effects),
        FocusTarget::Editor => handle_editor_key(app, key, effects),
        FocusTarget::Explorer => explorer_keys::handle_key(app, key, effects),
        FocusTarget::Tabs => opentabs::handle_key(app, key, effects),
        FocusTarget::Title => crate::title::handle_key(app, key, effects),
        FocusTarget::Messages => {
            if crate::messages::handle_key(app, key, effects) {
                keymap::KeyOutcome::Consumed
            } else {
                keymap::KeyOutcome::Ignored
            }
        }
    };
}

fn handle_editor_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> keymap::KeyOutcome {
    if crate::diff_view::keys::intercept(app, key) {
        return keymap::KeyOutcome::Consumed;
    }

    if crate::editor_fast_path::hardcoded_fast_path(app, key, effects) {
        return keymap::KeyOutcome::Consumed;
    }

    let Some(command) = keymap::resolve(key) else {
        if let KeyCode::Char(ch) = key.code {
            if is_insertable_key(key.mods, ch) {
                edit::insert_char(app, app.active, ch);
                return keymap::KeyOutcome::Consumed;
            }
            if key.mods.alt {
                crate::messages::warn_if_new(
                    app,
                    "\u{2325} chord not bound here \u{2014} nothing typed",
                );
                return keymap::KeyOutcome::Consumed;
            }
        }
        return keymap::KeyOutcome::Ignored;
    };

    editor_exec::run(app, command, QuitKey::from_key(key), effects)
}

// macOS composes an Option chord into text (⌥B into ∫) before any modifier
// survives a legacy (non-Kitty) terminal encoding, so a char-only filter has
// nothing left to refuse by the time the composed glyph arrives. Kitty's
// CSI-u protocol reports the same chord as the base key plus an explicit
// alt bit instead (requested via `term::keyboard_flags`), which is what
// lets `mods.alt` catch it here on any terminal that honours it.
fn is_insertable_key(mods: Mods, ch: char) -> bool {
    !mods.ctrl && !mods.alt && !mods.sup && !ch.is_control()
}
