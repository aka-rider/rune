use rune_syntax::DocumentKind;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::messages;
use crate::save::SaveStart;

pub(crate) struct SaveClearance(());

pub(crate) enum SaveEntry {
    Materialize,
    BindNew,
}

pub(crate) fn clear(
    app: &mut App,
    id: DocumentId,
    entry: SaveEntry,
) -> Result<SaveClearance, SaveStart> {
    match entry {
        SaveEntry::Materialize => materialize_rungs(app, id)?,
        SaveEntry::BindNew => bind_new_rungs(app, id)?,
    }
    Ok(SaveClearance(()))
}

fn materialize_rungs(app: &mut App, id: DocumentId) -> Result<(), SaveStart> {
    let Some(kind) = app.doc(id).map(|d| d.kind) else {
        messages::warn(app, "can't save \u{2014} that document is no longer open");
        return Err(SaveStart::Refused);
    };
    if kind == DocumentKind::Image {
        messages::warn(app, "images can't be edited or saved here");
        return Err(SaveStart::Refused);
    }
    refuse_while_saving(app, id)?;
    if app.rename.in_flight() {
        messages::error(app, "can't save while a rename is in flight");
        return Err(SaveStart::Refused);
    }
    refuse_while_merging(app, id)
}

fn bind_new_rungs(app: &mut App, id: DocumentId) -> Result<(), SaveStart> {
    refuse_while_saving(app, id)?;
    refuse_while_merging(app, id)
}

fn refuse_while_saving(app: &mut App, id: DocumentId) -> Result<(), SaveStart> {
    if !app.doc(id).is_some_and(Document::save_in_flight) {
        return Ok(());
    }
    messages::warn(app, "a save is already in progress");
    Err(SaveStart::InFlight)
}

fn refuse_while_merging(app: &mut App, id: DocumentId) -> Result<(), SaveStart> {
    if crate::merge::refuses_save(app, id) {
        return Err(SaveStart::Refused);
    }
    Ok(())
}
