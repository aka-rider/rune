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

pub(crate) fn tab_switch(app: &App) -> Availability {
    if app.documents.order().len() > 1 {
        Availability::Available
    } else {
        Availability::Unavailable("no other tab to switch to".into())
    }
}
