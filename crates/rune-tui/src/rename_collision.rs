use super::*;

/// Whether `[R]eplace` can be offered: it needs a durable store to capture
/// the displaced bytes into BEFORE they are destroyed. A
/// degraded store still counts — it is a live, if untrusted, connection,
/// and a `put_blob` that fails there surfaces as an ordinary `Err` edge
/// rather than a silent loss.
pub fn replace_allowed(app: &App) -> bool {
    let Some(doc_id) = collision_doc(app) else {
        return false;
    };
    app.db.is_some()
        && app
            .doc(doc_id)
            .is_some_and(crate::document::Document::is_store_bound)
}

/// `[R]eplace` was pressed and allowed. Clears the prompt, mints a fresh
/// ticket, and enqueues the one non-cancellable capture-then-swap op.
///
/// Writes no focus of its own — same reasoning as `bind_to`: by the time the
/// Collision Guard is even reachable, the blur that fired the original
/// commit already moved focus to the Editor, so there is nothing left here
/// to move.
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

    // Move to `Capturing` BEFORE clearing the modal: `clear_modal` calls
    // `on_prompt_dismissed`, which cancels a `Collision` — leaving the
    // order the other way round would immediately undo this confirmation.
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

/// Called by `guard::clear_guard` whenever a `RenameCollision` Guard is
/// removed — by `Esc`, by an `Error` displacing it, by anything at all.
/// Holds up the second half of the global invariant: no `Collision` state
/// ever outlives its prompt.
///
/// Returns the user to the title field with the name they typed still in
/// it, so a cancelled collision is one keystroke from a different name.
pub fn on_prompt_dismissed(app: &mut App) {
    let RenameState::Collision { doc, .. } = &app.rename else {
        return;
    };
    let doc = *doc;
    app.rename = RenameState::Idle;
    return_to_title(app, doc);
}
