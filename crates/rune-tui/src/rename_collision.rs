use super::*;

pub fn replace_allowed(app: &App) -> bool {
    let Some(doc_id) = collision_doc(app) else {
        return false;
    };
    app.db.is_some()
        && app
            .doc(doc_id)
            .is_some_and(crate::document::Document::is_store_bound)
}

pub fn replace_confirmed(app: &mut App) {
    let RenameState::Collision {
        doc,
        from,
        to,
        seen,
    } = app.rename.clone()
    else {
        return;
    };

    let Some(db_id) = app.doc(doc).and_then(|d| d.doc_db()).map(|d| d.db_id) else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };

    match db
        .store
        .rename_replace(rune_db::DocId(db_id), &from, &to, seen)
    {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(doc));
            // Move to `Capturing` BEFORE clearing the guard: `clear_guard`
            // calls `on_prompt_dismissed`, which cancels a `Collision` —
            // the reverse order would immediately undo this confirmation.
            app.rename = RenameState::Capturing {
                doc,
                from,
                to,
                seen,
                ticket: Ticket::Db(op_id),
            };
            guard::clear_guard(app);
        }
        Err(e) => {
            app.rename = RenameState::Idle;
            guard::clear_guard(app);
            crate::materialize_ack::on_store_failure(app, &e.to_string());
        }
    }
}

pub fn on_prompt_dismissed(app: &mut App) {
    let RenameState::Collision { doc, .. } = &app.rename else {
        return;
    };
    let doc = *doc;
    app.rename = RenameState::Idle;
    return_to_title(app, doc);
}
