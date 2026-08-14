//! `⌘⌫`/`^⌫` — moves the Explorer-selected file or the active document's
//! file to the OS Trash, behind a `guard::GuardKind::Trash` confirm
//! prompt. `request_trash` resolves the target and raises the guard;
//! `confirm` enqueues the off-thread `vfs.trash` call once the user answers
//! `[Y]es`; `handle_trash_done` reacts to its reply.
//!
//! Path matching against open documents is exact `PathBuf` equality
//! (`workspace::existing_document_for`) — a file opened under one spelling
//! and selected under another (a symlink, a non-canonical prefix) is not
//! recognized as the same document. Inherited from the rest of the
//! workspace's document-identity matching, not introduced here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_vfs::Vfs;

use crate::app::App;
use crate::document::DocumentId;
use crate::explorer;
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::materialize_ack;
use crate::messages;
use crate::pane::Pane;
use crate::runtime::{Cmd, Effects, Msg};
use crate::workspace;

/// Resolves the trash target from the current focus and raises the confirm
/// guard. Explorer focus reads the selection the way `explorer_keys::
/// open_selected` does; any other focus targets the active document's own
/// file. Refuses (with a `messages::error`, never a silent no-op) a
/// directory selection, a pathless draft, and a dirty open document — the
/// last re-checked again at `confirm` and once more when the reply lands,
/// since the user can keep typing between each of these points.
pub(crate) fn request_trash(app: &mut App, _effects: &mut Effects) {
    if app.trash_pending.is_some() {
        messages::error(app, "a trash is already in progress");
        return;
    }

    let target = if app.focus() == Pane::Explorer {
        let Some((path, is_dir)) = app
            .explorer
            .entries
            .get(app.explorer.nav.cursor)
            .map(|e| (e.path.clone(), e.kind == rune_vfs::FileKind::Dir))
        else {
            messages::error(app, "nothing to trash \u{2014} no file selected");
            return;
        };
        if is_dir {
            messages::error(app, "cannot trash a directory");
            return;
        }
        path
    } else {
        let Some(path) = app.active_doc().file_path.clone() else {
            messages::error(app, "nothing to trash \u{2014} draft has no file");
            return;
        };
        path
    };

    let Some(path) = workspace::resolve_or_report(app, &target, "trash") else {
        return;
    };
    if refuse_if_dirty(app, &path) {
        return;
    }

    let _ = guard::set_guard_or_warn(
        app,
        GuardPrompt {
            doc: app.active,
            kind: GuardKind::Trash { path },
        },
        "trash confirmation dropped \u{2014} a prompt is already showing",
    );
}

/// The trash guard's `[Y]es` answer: refuses a second commit while one is
/// already in flight (mirrors `rename::begin`'s `in_flight` refusal),
/// re-runs the dirty refusal (the user may have edited the open document
/// between the chord and the confirm), then enqueues the off-thread `vfs.
/// trash` call under a freshly minted generation and records it in `app.
/// trash_pending`.
pub(crate) fn confirm(app: &mut App, path: PathBuf, effects: &mut Effects) {
    if app.trash_pending.is_some() {
        messages::error(app, "a trash is already in progress");
        return;
    }
    if refuse_if_dirty(app, &path) {
        return;
    }
    app.trash_gen = app.trash_gen.wrapping_add(1);
    let generation = app.trash_gen;
    app.trash_pending = Some(path.clone());
    effects
        .cmds
        .push(trash_cmd(Arc::clone(&app.vfs), path, generation));
}

/// Refuses (with a `messages::error`) a trash target whose document is open
/// and dirty right now — shared by `request_trash`'s initial check and
/// `confirm`'s re-check, since the user can keep typing between the two.
fn refuse_if_dirty(app: &mut App, path: &Path) -> bool {
    if let Some(id) = workspace::existing_document_for(app, path)
        && materialize_ack::is_dirty_now(app, id)
    {
        messages::error(app, "unsaved changes \u{2014} save before trashing");
        return true;
    }
    false
}

/// The off-thread `vfs.trash` call — mirrors `rename_create::rename_cmd`'s
/// shape.
fn trash_cmd(vfs: Arc<dyn Vfs + Send + Sync>, path: PathBuf, generation: u32) -> Cmd {
    Cmd::trash(move || {
        let result = vfs.trash(&path).map_err(|e| e.to_string());
        Some(Msg::TrashDone {
            generation,
            path,
            result,
        })
    })
}

/// `Msg::TrashDone`'s handler. A stale `generation` (a fresh trash request
/// started and finished before this one's reply lands) is dropped on
/// arrival before `app.trash_pending` is touched — under single-flight
/// enforcement (`request_trash`/`confirm` both refuse while it is `Some`)
/// this reply can only be stale for a generation that predates the one
/// `app.trash_pending` currently names, so it owns none of the state there
/// is to clear. Once a reply IS for the current generation, `app.
/// trash_pending` is cleared unconditionally — before the `Ok`/`Err` match,
/// so neither outcome can leave the next request refused forever. `Err` is
/// reported and closes nothing. `Ok` re-derives dirtiness one last time
/// (assumption A4): a document that became dirty while the Cmd was in
/// flight keeps its tab open (the file is gone, but the unsaved words are
/// not) and a single Warn message says so; otherwise the tab is closed and
/// a single Info message says so — exactly one message per outcome. Any
/// Guard still live for the closing document (`close_now` does not sweep
/// `app.guard`) is cleared first, or the footer would go on rendering a
/// prompt for a document that no longer exists.
pub(crate) fn handle_trash_done(
    app: &mut App,
    generation: u32,
    path: &Path,
    result: Result<(), String>,
    effects: &mut Effects,
) {
    if generation != app.trash_gen {
        return;
    }
    app.trash_pending = None;
    let name = display_name(path);
    match result {
        Err(e) => {
            messages::error(app, format!("trash failed: {e}"));
        }
        Ok(()) => {
            let mut kept_open = false;
            if let Some(id) = workspace::existing_document_for(app, path) {
                sweep_live_guard(app, id);
                if materialize_ack::is_dirty_now(app, id) {
                    kept_open = true;
                    messages::warn(
                        app,
                        format!(
                            "{name} moved to Trash \u{2014} unsaved changes kept in the open tab"
                        ),
                    );
                } else {
                    let _ = workspace::close_now(app, id, effects);
                }
            }
            explorer::refresh_for(app, path, effects);
            if !kept_open {
                messages::info(app, format!("moved to Trash: {name}"));
            }
        }
    }
}

/// Clears a live Guard prompting for `doc` before it is closed out from
/// under the user — `close_now` never sweeps `app.guard` itself.
fn sweep_live_guard(app: &mut App, doc: DocumentId) {
    if app.guard.as_ref().is_some_and(|p| p.doc == doc) {
        guard::clear_guard(app);
        messages::info(app, "prompt dismissed \u{2014} file was trashed");
    }
}

/// The display name for a trash target: the file name, or the whole path
/// when it has none (e.g. a root). Shared by the guard footer's prompt and
/// this module's own `Ok`/`Err` messages.
pub(crate) fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::guard::{GuardRaise, set_guard};
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.active_doc_mut().file_path = Some(PathBuf::from("/doc.md"));
        app
    }

    #[test]
    fn a_resolve_failing_target_aborts_the_trash_request_and_posts_a_message() {
        let mem = Arc::new(Mem::new());
        mem.fail_resolve(Path::new("/doc.md"));
        let vfs: Arc<dyn Vfs + Send + Sync> = mem.clone();
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        app.active_doc_mut().file_path = Some(PathBuf::from("/doc.md"));
        let mut effects = Effects::default();

        request_trash(&mut app, &mut effects);

        assert!(
            app.guard.is_none(),
            "a resolve failure must never raise the trash guard"
        );
        assert!(
            messages::newest_text(&app).is_some(),
            "a resolve failure must post a message"
        );
    }

    /// The trash chord, resolved through `App::update`'s real `Msg::Key`
    /// dispatch, never reaches `request_trash` at all while a Guard is
    /// already showing (`dispatch::handle_key`'s Stage 1 routes every key
    /// to the existing prompt first) — so a foreign Guard already up is
    /// exercised by calling `request_trash` directly, exactly the real
    /// entry point `GlobalCommand::Trash` resolves to; the two paths only
    /// ever differ by that already-showing-prompt short circuit, never by
    /// what `request_trash` itself does. Raising `Trash` against an
    /// occupied slot must warn and leave the original prompt alone, rather
    /// than silently dropping the trash intent.
    #[test]
    fn trash_chord_while_a_different_guard_is_up_warns_and_preserves_it() {
        let mut app = app();
        let doc = app.active;
        assert_eq!(
            set_guard(
                &mut app,
                GuardPrompt {
                    doc,
                    kind: GuardKind::DiskConflict,
                },
            ),
            GuardRaise::Raised,
            "test setup: pre-arm a foreign guard"
        );

        let mut effects = Effects::default();
        request_trash(&mut app, &mut effects);

        assert!(
            matches!(
                app.guard,
                Some(GuardPrompt {
                    kind: GuardKind::DiskConflict,
                    ..
                })
            ),
            "the pre-existing prompt must survive unchanged"
        );
        assert!(app.trash_pending.is_none(), "no trash was armed");
        assert_eq!(
            messages::newest_text(&app),
            Some("trash confirmation dropped \u{2014} a prompt is already showing")
        );
    }
}
