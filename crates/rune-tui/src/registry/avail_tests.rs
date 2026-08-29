#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use crate::app::App;
use crate::document::ReadOnly;
use crate::global::GlobalCommand;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::runtime::Effects;

use super::{Availability, CommandId, spec};

fn app_with(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    let id = app.active;
    app.doc_mut(id)
        .expect("fixture doc must exist")
        .viewport
        .set_size(80, 23);
    app
}

fn row_position(app: &App, name: &str) -> usize {
    app.palette()
        .expect("palette must be open")
        .rows
        .iter()
        .position(|row| spec(row.id).is_some_and(|s| s.name == name))
        .expect("no palette row with this name")
}

fn row_availability(app: &App, name: &str) -> Availability {
    app.palette()
        .expect("palette must be open")
        .rows
        .iter()
        .find(|row| spec(row.id).is_some_and(|s| s.name == name))
        .expect("no palette row with this name")
        .availability
        .clone()
}

fn unavailable_reason(availability: Availability) -> String {
    match availability {
        Availability::Unavailable(reason) => Some(reason.into_owned()),
        Availability::Available => None,
    }
    .expect("expected an unavailable row")
}

#[test]
fn read_only_document_greys_cut_and_uppercase_below_available_rows() {
    let mut app = app_with("hello world");
    app.active_doc_mut().read_only = ReadOnly::Reading;
    let mut effects = Effects::default();
    crate::palette::open(&mut app, &mut effects);

    assert!(matches!(
        row_availability(&app, "cut"),
        Availability::Unavailable(_)
    ));
    assert!(matches!(
        row_availability(&app, "uppercase"),
        Availability::Unavailable(_)
    ));
    assert_eq!(row_availability(&app, "save"), Availability::Available);

    let cut_idx = row_position(&app, "cut");
    let uppercase_idx = row_position(&app, "uppercase");
    let save_idx = row_position(&app, "save");
    assert!(
        cut_idx > save_idx,
        "an unavailable row must sort below an available one"
    );
    assert!(
        uppercase_idx > save_idx,
        "an unavailable row must sort below an available one"
    );

    if let Some(state) = app.palette_mut() {
        state.nav.cursor = cut_idx;
    }
    let version_before = app.active_doc().buffer.version();
    let enter = KeyInput {
        code: KeyCode::Enter,
        mods: Mods::NONE,
    };
    let _ = crate::palette::keys::handle_key(&mut app, enter, &mut effects);

    assert_eq!(app.active_doc().buffer.version(), version_before);
    assert_eq!(
        app.palette().and_then(|s| s.refusal.clone()),
        ReadOnly::Reading.refusal_message().map(str::to_string)
    );
    assert!(
        app.palette().is_some(),
        "a refusal never closes the palette"
    );
}

#[test]
fn merge_refusal_matches_across_chord_palette_and_footer() {
    let mut app = app_with("hello");
    let mut effects = Effects::default();

    let entries_before = crate::footer_hints::default_hint_entries(&app);
    assert!(
        !entries_before.iter().any(|(_, help, _)| *help == "merge"),
        "the footer must not offer a merge hint with nothing to merge"
    );

    let ctrl_m = KeyInput {
        code: KeyCode::Char('m'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    };
    crate::dispatch::handle_key(&mut app, ctrl_m, &mut effects);
    let chord_reason = crate::messages::newest_text(&app)
        .expect("the chord must post a refusal")
        .to_string();

    let predicate_reason = unavailable_reason(super::availability(
        &app,
        CommandId::Global(GlobalCommand::Merge),
    ));
    assert_eq!(chord_reason, predicate_reason);

    crate::palette::open(&mut app, &mut effects);
    let palette_reason = unavailable_reason(row_availability(&app, "merge"));
    assert_eq!(chord_reason, palette_reason);
}

#[test]
fn reload_refusal_matches_between_the_chord_and_the_palette() {
    let mut app = app_with("hello");
    let mut effects = Effects::default();
    assert!(!app.active_doc().has_reloadable_graphics());

    let cmd_r = KeyInput {
        code: KeyCode::Char('r'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    };
    crate::dispatch::handle_key(&mut app, cmd_r, &mut effects);
    let chord_reason = crate::messages::newest_text(&app)
        .expect("the chord must post a refusal")
        .to_string();
    assert_eq!(chord_reason, "nothing to reload");

    crate::palette::open(&mut app, &mut effects);
    let reload_idx = row_position(&app, "reload graphics");
    if let Some(state) = app.palette_mut() {
        state.nav.cursor = reload_idx;
    }
    let enter = KeyInput {
        code: KeyCode::Enter,
        mods: Mods::NONE,
    };
    let _ = crate::palette::keys::handle_key(&mut app, enter, &mut effects);
    assert_eq!(
        app.palette().and_then(|s| s.refusal.clone()),
        Some(chord_reason)
    );
}

#[test]
fn save_in_flight_refusal_matches_between_chord_and_palette() {
    let mut app = app_with("hello");
    let mut effects = Effects::default();
    let version = app.active_doc().buffer.version();
    let content: Arc<str> = Arc::from(app.active_doc().buffer.content());
    app.active_doc_mut().begin_save(version, content);

    let ctrl_s = KeyInput {
        code: KeyCode::Char('s'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    };
    crate::dispatch::handle_key(&mut app, ctrl_s, &mut effects);
    let chord_reason = crate::messages::newest_text(&app)
        .expect("the chord must post a refusal")
        .to_string();
    assert_eq!(chord_reason, "a save is already in progress");

    let predicate_reason = unavailable_reason(super::availability(
        &app,
        CommandId::Global(GlobalCommand::Save),
    ));
    assert_eq!(chord_reason, predicate_reason);

    crate::palette::open(&mut app, &mut effects);
    let palette_reason = unavailable_reason(row_availability(&app, "save"));
    assert_eq!(chord_reason, palette_reason);
}

#[test]
fn focus_title_stays_available_while_a_rename_is_in_flight() {
    let mut app = app_with("hello");
    let doc = app.active;
    app.rename = crate::rename::RenameState::Committing {
        doc,
        from: std::path::PathBuf::from("/a.md"),
        to: std::path::PathBuf::from("/b.md"),
        ticket: crate::rename::Ticket::Cmd(app.next_rename_gen.mint()),
        draft_baseline: None,
    };

    assert!(matches!(
        super::availability(&app, CommandId::Global(GlobalCommand::FocusTitle)),
        crate::registry::Availability::Available
    ));
}

#[test]
fn close_while_saving_still_defers_instead_of_refusing() {
    let mut app = app_with("hello");
    let mut effects = Effects::default();
    let version = app.active_doc().buffer.version();
    let content: Arc<str> = Arc::from(app.active_doc().buffer.content());
    app.active_doc_mut().begin_save(version, content);

    let ctrl_w = KeyInput {
        code: KeyCode::Char('w'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    };
    crate::dispatch::handle_key(&mut app, ctrl_w, &mut effects);

    assert!(
        app.doc(app.active).is_some(),
        "the close must wait for the save's ack"
    );
    assert_eq!(app.pending_close_on_save, Some(app.active));
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("save in progress \u{2014} closing once it completes")
    );
}

#[test]
fn palette_availability_goes_stale_free_when_a_stage_two_chord_flips_read_only() {
    let mut app = app_with("hello world");
    let mut effects = Effects::default();
    crate::palette::open(&mut app, &mut effects);

    assert_eq!(row_availability(&app, "uppercase"), Availability::Available);

    let ctrl_shift_p = KeyInput {
        code: KeyCode::Char('P'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    };
    crate::app::update(
        &mut app,
        crate::runtime::Msg::Key(ctrl_shift_p),
        &mut effects,
    );

    assert_eq!(app.active_doc().read_only, ReadOnly::Reading);
    assert!(
        app.palette().is_some(),
        "^⇧P leaves the palette open (LeaveOpen policy)"
    );
    assert!(matches!(
        row_availability(&app, "uppercase"),
        Availability::Unavailable(_)
    ));
}
