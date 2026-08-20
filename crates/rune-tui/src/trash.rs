use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_vfs::Vfs;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::explorer;
use crate::generation::TrashGen;
use crate::guard::{self, GuardKind, GuardPrompt, TrashSubject};
use crate::messages;
use crate::pane::Pane;
use crate::runtime::{Cmd, CmdError, Effects, Msg};
use crate::workspace;

#[derive(Default)]
pub(crate) enum TrashState {
    #[default]
    Idle,
    Pending { generation: TrashGen },
}

pub(crate) fn request_trash(app: &mut App, effects: &mut Effects) {
    if !matches!(app.trash, TrashState::Idle) {
        messages::error(app, "a trash is already in progress");
        return;
    }

    let Some((path, subject)) = target(app) else {
        return;
    };
    if refuse_if_dirty(app, &path) {
        return;
    }

    let _ = guard::set_guard_or_warn(
        app,
        GuardPrompt {
            doc: app.active,
            kind: GuardKind::Trash { path, subject },
        },
        "trash confirmation dropped \u{2014} a prompt is already showing",
        effects,
    );
}

fn target(app: &mut App) -> Option<(PathBuf, TrashSubject)> {
    if app.focus() == Pane::Explorer {
        return selected_row_target(app);
    }
    let Some(path) = app.active_doc().file_path.clone() else {
        messages::error(app, "nothing to trash \u{2014} draft has no file");
        return None;
    };
    let resolved = workspace::resolve_or_report(app, &path, "trash")?;
    Some((resolved, TrashSubject::File))
}

fn selected_row_target(app: &mut App) -> Option<(PathBuf, TrashSubject)> {
    let row = app
        .explorer
        .entries
        .get(app.explorer.nav.cursor)
        .map(|e| (e.path.clone(), e.kind, e.link));
    let Some((path, kind, link)) = row else {
        messages::error(app, "nothing to trash \u{2014} no file selected");
        return None;
    };
    match link {
        rune_vfs::Link::To | rune_vfs::Link::Broken => Some((path, TrashSubject::Symlink)),
        rune_vfs::Link::No if kind == rune_vfs::FileKind::Dir => {
            messages::error(app, "cannot trash a directory");
            None
        }
        rune_vfs::Link::No => Some((path, TrashSubject::File)),
    }
}

pub(crate) fn confirm(app: &mut App, path: PathBuf, effects: &mut Effects) {
    if !matches!(app.trash, TrashState::Idle) {
        messages::error(app, "a trash is already in progress");
        return;
    }
    if refuse_if_dirty(app, &path) {
        return;
    }
    let generation = app.next_trash_gen.mint();
    app.trash = TrashState::Pending { generation };
    effects
        .cmds
        .push(trash_cmd(Arc::clone(&app.vfs), path, generation));
}

fn refuse_if_dirty(app: &mut App, path: &Path) -> bool {
    if let Some(id) = workspace::existing_document_for(app, path)
        && app.doc(id).is_some_and(Document::is_dirty)
    {
        messages::error(app, "unsaved changes \u{2014} save before trashing");
        return true;
    }
    false
}

fn trash_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    generation: crate::generation::TrashGen,
) -> Cmd {
    Cmd::trash(move || {
        let result = vfs.trash(&path).map_err(CmdError::from);
        Some(Msg::TrashDone {
            generation,
            path,
            result,
        })
    })
}

pub(crate) fn handle_trash_done(
    app: &mut App,
    generation: crate::generation::TrashGen,
    path: &Path,
    result: Result<(), CmdError>,
    effects: &mut Effects,
) {
    let TrashState::Pending { generation: expected } = &app.trash else {
        return;
    };
    if generation != *expected {
        return;
    }
    app.trash = TrashState::Idle;
    let name = display_name(path);
    match result {
        Err(e) => {
            messages::error(app, format!("trash failed: {e}"));
        }
        Ok(()) => {
            let mut kept_open = false;
            if let Some(id) = workspace::existing_document_for(app, path) {
                sweep_live_guard(app, id);
                if app.doc(id).is_some_and(Document::is_dirty) {
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

fn sweep_live_guard(app: &mut App, doc: DocumentId) {
    if app.guard.as_ref().is_some_and(|p| p.doc == doc) {
        guard::clear_guard(app);
        messages::info(app, "prompt dismissed \u{2014} file was trashed");
    }
}

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
                &mut crate::runtime::Effects::default(),
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
        assert!(matches!(app.trash, TrashState::Idle), "no trash was armed");
        assert_eq!(
            messages::newest_text(&app),
            Some("trash confirmation dropped \u{2014} a prompt is already showing")
        );
    }
}
