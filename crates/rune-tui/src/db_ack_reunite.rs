use super::detach_file_binding;
use crate::app::App;
use crate::db::LoadPurpose;
use crate::document::{Document, DocumentId};
use crate::messages;
use rune_db::LoadResult;

/// An external rename O -> N with a tab still open at O makes opening N
/// mint a duplicate: the fresh document is bound to N, while the store
/// finds the same file by inode and reports `renamed_from = O`. The tab at
/// O carries the user's undo history, so it follows its file to N and the
/// just-minted duplicate retires — unless either side holds anything
/// unsettled, in which case nothing is touched and both tabs stay.
pub(super) fn reunite_external_rename(
    app: &mut App,
    id: DocumentId,
    load_result: &LoadResult,
    purpose: LoadPurpose,
    effects: &mut crate::runtime::Effects,
) -> bool {
    let Some(renamed_from) = load_result.renamed_from.as_deref() else {
        return false;
    };
    if !matches!(purpose, LoadPurpose::Recover) {
        return false;
    }
    let old_path = std::path::Path::new(renamed_from);
    let Some(new_path) = app.doc(id).and_then(Document::resolved_path).cloned() else {
        return false;
    };
    if new_path.as_path() == old_path {
        return false;
    }
    let Some(prior) = app
        .documents
        .iter()
        .find(|(other, doc)| **other != id && doc.path() == Some(old_path))
        .map(|(other, _)| *other)
    else {
        return false;
    };
    let (old_label, new_label) = rename_labels(old_path, new_path.as_path());
    let fresh_untouched = app
        .doc(id)
        .is_some_and(|doc| !doc.is_dirty() && !doc.save_in_flight())
        && app.rename.doc() != Some(id);
    let prior_settled = app
        .doc(prior)
        .is_some_and(|doc| !doc.is_dirty() && !doc.save_in_flight())
        && app.merge.doc() != Some(prior);
    if !fresh_untouched || !prior_settled {
        messages::warn(
            app,
            format!(
                "{old_label} was renamed to {new_label} on disk \u{2014} it is open in another tab with unsettled changes, so both tabs were kept"
            ),
        );
        return false;
    }
    app.rebind_document_path(prior, new_path.clone());
    let _ = crate::workspace::close_now(app, id, effects);
    crate::workspace::switch_to(app, prior);
    if !crate::db_enqueue::load_document_best_effort(app, prior, &new_path) {
        detach_file_binding(app, prior);
    }
    messages::info(
        app,
        format!("{old_label} was renamed to {new_label} on disk \u{2014} its open tab followed"),
    );
    true
}

fn rename_labels(old: &std::path::Path, new: &std::path::Path) -> (String, String) {
    let name = |path: &std::path::Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    match (name(old), name(new)) {
        (Some(old_name), Some(new_name)) if old_name != new_name => (old_name, new_name),
        _ => (old.display().to_string(), new.display().to_string()),
    }
}
