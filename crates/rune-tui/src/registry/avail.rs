use rune_syntax::DocumentKind;

use crate::app::App;
use crate::document::ReadOnly;

use super::Availability;

pub(crate) fn always(_app: &App) -> Availability {
    Availability::Available
}

pub(crate) fn read_only_edit(app: &App) -> Availability {
    match app.active_doc().read_only.refusal_message() {
        Some(reason) => Availability::Unavailable(reason.into()),
        None => Availability::Available,
    }
}

pub(crate) fn toggle_read_only(app: &App) -> Availability {
    if app.active_doc().read_only == ReadOnly::Always {
        Availability::Unavailable(
            ReadOnly::Always
                .refusal_message()
                .unwrap_or_default()
                .into(),
        )
    } else {
        Availability::Available
    }
}

pub(crate) fn preview_locked(app: &App) -> Availability {
    if app.active_doc().read_only == ReadOnly::Preview {
        Availability::Unavailable(
            ReadOnly::Preview
                .refusal_message()
                .unwrap_or_default()
                .into(),
        )
    } else {
        Availability::Available
    }
}

pub(crate) fn merge(app: &App) -> Availability {
    let session_exists_here = app.merge.doc() == Some(app.active);
    if session_exists_here || crate::merge::is_divergent(app.active_doc()) {
        Availability::Available
    } else {
        Availability::Unavailable(crate::merge::NO_DIVERGENCE_REASON.into())
    }
}

pub(crate) fn reload(app: &App) -> Availability {
    if app.active_doc().has_reloadable_graphics() {
        Availability::Available
    } else {
        Availability::Unavailable("nothing to reload".into())
    }
}

pub(crate) fn language(app: &App) -> Availability {
    if app.active_doc().kind == DocumentKind::Image {
        Availability::Unavailable("not available for an image".into())
    } else {
        Availability::Available
    }
}

/// `GlobalCommand::Save`'s row — a read-only mirror of `save::gate::
/// materialize_rungs`'s own preconditions for `app.active`, in the same
/// order, so the palette greys the row for exactly the reasons a direct
/// `^S` would refuse for. The merge-blocks-save rung is deliberately NOT
/// mirrored here (mirrors `merge` above, which is its own row for its own
/// command): its wording is built at refusal time from live state
/// (`unresolved_count`, a save-key label), not a fixed string, and posting
/// it needs `&mut App`.
pub(crate) fn save(app: &App) -> Availability {
    if app.active_doc().kind == DocumentKind::Image {
        return Availability::Unavailable("images can't be edited or saved here".into());
    }
    let preview = preview_locked(app);
    if !matches!(preview, Availability::Available) {
        return preview;
    }
    if app.active_doc().save_in_flight() {
        return Availability::Unavailable("a save is already in progress".into());
    }
    if app.rename.in_flight() {
        return Availability::Unavailable("can't save while a rename is in flight".into());
    }
    Availability::Available
}

/// `GlobalCommand::FocusTitle`'s row (labeled "rename") — a read-only
/// mirror of `rename::begin`'s own in-flight rungs, in the same order,
/// layered onto the pre-existing `preview_locked` gate this row already
/// carried. Catching these BEFORE the title ever gets focus is strictly
/// better than the old behavior (focus, type, then get refused on blur):
/// the same two reasons `rename::begin` posts at commit time, surfaced the
/// moment the chord is pressed instead.
pub(crate) fn rename(app: &App) -> Availability {
    preview_locked(app)
}

pub(crate) fn tab_switch(app: &App) -> Availability {
    if app.documents.order().len() > 1 {
        Availability::Available
    } else {
        Availability::Unavailable("no other tab to switch to".into())
    }
}
