#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use crate::app::App;
use crate::global::GlobalCommand;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::registry::{Availability, CommandId};
use crate::runtime::Effects;

use super::*;

fn app() -> App {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    app.frame_width = 100;
    app.frame_height = 30;
    app
}

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

fn ctrl_shift_p() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('P'),
        mods: CTRL,
    }
}

#[test]
fn the_toggle_chord_opens_the_palette() {
    let mut app = app();
    let mut effects = Effects::default();
    crate::dispatch::handle_key(&mut app, ctrl_shift_p(), &mut effects);
    assert!(app.palette().is_some());
}

#[test]
fn escape_closes_the_palette_and_leaves_prior_state_untouched() {
    let mut app = app();
    let before_focus = app.focus();
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    crate::dispatch::handle_key(&mut app, ctrl_shift_p(), &mut effects);
    assert!(app.palette().is_some());
    assert!(
        app.search().is_none(),
        "opening the palette closes an open search bar"
    );

    let escape = KeyInput {
        code: KeyCode::Escape,
        mods: Mods::NONE,
    };
    crate::dispatch::handle_key(&mut app, escape, &mut effects);

    assert!(app.palette().is_none());
    assert_eq!(app.focus(), before_focus);
}

#[test]
fn opening_the_palette_while_filesearch_is_open_closes_filesearch() {
    let mut app = app();
    let mut effects = Effects::default();
    crate::filesearch::open(&mut app, &mut effects);
    assert!(app.filesearch().is_some(), "test setup");

    open(&mut app, &mut effects);

    assert!(app.palette().is_some());
    assert!(app.filesearch().is_none());
}

#[test]
fn a_query_hitting_help_text_only_ranks_below_a_name_hit() {
    let mut app = app();
    let mut effects = Effects::default();
    crate::pane_global::new_document(&mut app, &mut effects);
    open(&mut app, &mut effects);
    if let Some(state) = app.palette_mut() {
        state.field.set_text("e");
    }
    recompute(&mut app);

    let state = app.palette().expect("open");
    let tab_row = state
        .rows
        .iter()
        .find(|row| crate::registry::spec(row.id).is_some_and(|s| s.name == "tab"))
        .expect("\"tab\" must still surface on a query matched only through its help text");
    assert_eq!(
        tab_row.tier,
        Tier::HelpHit,
        "\"tab\" has no \"e\" in its name; a hit here must come from its help text, or this test is vacuous"
    );

    let save_pos = state
        .rows
        .iter()
        .position(|row| crate::registry::spec(row.id).is_some_and(|s| s.name == "save"))
        .expect("save row present");
    let tab_pos = state
        .rows
        .iter()
        .position(|row| crate::registry::spec(row.id).is_some_and(|s| s.name == "tab"))
        .expect("tab row present");
    assert!(
        tab_pos > save_pos,
        "a help-only hit must rank below a name hit"
    );
}

#[test]
fn enter_on_save_produces_the_same_state_as_the_direct_save_chord() {
    let mut app_a = app();
    let mut effects_a = Effects::default();
    let ctrl_s = KeyInput {
        code: KeyCode::Char('s'),
        mods: CTRL,
    };
    crate::dispatch::handle_key(&mut app_a, ctrl_s, &mut effects_a);

    let mut app_b = app();
    let mut effects_b = Effects::default();
    open(&mut app_b, &mut effects_b);
    if let Some(state) = app_b.palette_mut() {
        state.field.set_text("save");
    }
    recompute(&mut app_b);
    let cursor = app_b
        .palette()
        .expect("open")
        .rows
        .iter()
        .position(|row| crate::registry::spec(row.id).is_some_and(|s| s.name == "save"))
        .expect("save row present");
    if let Some(state) = app_b.palette_mut() {
        state.nav.cursor = cursor;
    }
    let enter = KeyInput {
        code: KeyCode::Enter,
        mods: Mods::NONE,
    };
    let _ = crate::palette::keys::handle_key(&mut app_b, enter, &mut effects_b);

    assert!(app_b.palette().is_none(), "Enter closes the palette");
    assert_eq!(
        app_a.active_doc().save_in_flight(),
        app_b.active_doc().save_in_flight()
    );
}

#[test]
fn enter_on_an_unavailable_row_sets_a_refusal_and_does_not_execute() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let docs_before = app.documents.len();
    if let Some(state) = app.palette_mut() {
        state.rows = vec![PaletteRow {
            id: CommandId::Global(GlobalCommand::NewDocument),
            via_alias: None,
            indices: Vec::new(),
            availability: Availability::Unavailable(std::borrow::Cow::Borrowed("refused")),
            tier: Tier::Unavailable,
        }];
        state.nav.cursor = 0;
    }
    let enter = KeyInput {
        code: KeyCode::Enter,
        mods: Mods::NONE,
    };
    let _ = crate::palette::keys::handle_key(&mut app, enter, &mut effects);

    assert!(
        app.palette().is_some(),
        "an unavailable row never closes the palette"
    );
    assert_eq!(
        app.palette().and_then(|s| s.refusal.clone()),
        Some("refused".to_string())
    );
    assert_eq!(app.documents.len(), docs_before, "the command must not run");
}

#[test]
fn a_stale_recents_reply_generation_is_dropped() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let current = app.palette().expect("open").generation;
    let stale = crate::generation::Generation::from_raw(current.raw().wrapping_add(1));
    handle_recents_loaded(&mut app, stale, Ok(vec!["save".to_string()]));
    assert!(
        app.palette().is_some_and(|s| s.recents.is_empty()),
        "a stale generation's reply must not land"
    );
}

#[test]
fn recents_drop_unknown_stored_names() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.palette().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec!["save".to_string(), "not-a-real-command".to_string()]),
    );
    let state = app.palette().expect("open");
    assert_eq!(
        state.recents,
        vec!["save".to_string(), "not-a-real-command".to_string()]
    );
    assert!(
        state.rows.iter().any(|row| row.tier == Tier::Recent
            && crate::registry::spec(row.id).is_some_and(|s| s.name == "save")),
        "the known recent name must resolve to a registry row"
    );
    assert!(
        !state.rows.iter().any(|row| row.tier == Tier::Recent
            && crate::registry::spec(row.id).is_some_and(|s| s.name == "not-a-real-command")),
        "an unknown stored name must be dropped from the recents rows"
    );
}

fn cursor_on(app: &mut App, name: &str) {
    let msg = format!("no row named {name:?}");
    let idx = app
        .palette()
        .expect("open")
        .rows
        .iter()
        .position(|row| crate::registry::spec(row.id).is_some_and(|s| s.name == name))
        .expect(&msg);
    if let Some(state) = app.palette_mut() {
        state.nav.cursor = idx;
    }
}

fn cursor_on_arg(app: &mut App, label: &str) {
    let msg = format!("no argument row labeled {label:?}");
    let idx = app
        .palette()
        .expect("open")
        .arg_rows
        .iter()
        .position(|row| row.label == label)
        .expect(&msg);
    if let Some(state) = app.palette_mut() {
        state.nav.cursor = idx;
    }
}

const TAB: KeyInput = KeyInput {
    code: KeyCode::Tab,
    mods: Mods::NONE,
};
const ENTER: KeyInput = KeyInput {
    code: KeyCode::Enter,
    mods: Mods::NONE,
};
const BACKSPACE: KeyInput = KeyInput {
    code: KeyCode::Backspace,
    mods: Mods::NONE,
};

#[test]
fn a_query_matching_only_an_alias_surfaces_its_row_via_the_alias() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    if let Some(state) = app.palette_mut() {
        state.field.set_text("syntax");
    }
    recompute(&mut app);

    let state = app.palette().expect("open");
    let row = state
        .rows
        .iter()
        .find(|row| crate::registry::spec(row.id).is_some_and(|s| s.name == "language"))
        .expect("\"syntax\" must surface the \"language\" row via its alias");
    assert_eq!(row.via_alias, Some("syntax"));
    assert!(
        row.indices.iter().all(|&i| (i as usize) < "syntax".len()),
        "the needle indices must point inside the matched alias, not the name"
    );
    assert!(!row.indices.is_empty());
}

#[test]
fn tab_on_an_unavailable_name_row_refuses_and_stays_in_name_mode() {
    let mut app = app();
    let mut effects = Effects::default();
    app.active_doc_mut().kind = rune_syntax::DocumentKind::Image;
    open(&mut app, &mut effects);
    cursor_on(&mut app, "language");

    let _ = crate::palette::keys::handle_key(&mut app, TAB, &mut effects);

    let state = app.palette().expect("open");
    assert_eq!(
        state.mode,
        PaletteMode::Name,
        "Tab must not enter Param mode on a refused row"
    );
    assert!(
        state.refusal.is_some(),
        "Tab on an unavailable row must set a refusal"
    );
}

#[test]
fn enter_on_an_unavailable_parameterized_row_refuses_and_stays_in_name_mode() {
    let mut app = app();
    let mut effects = Effects::default();
    app.active_doc_mut().kind = rune_syntax::DocumentKind::Image;
    open(&mut app, &mut effects);
    cursor_on(&mut app, "language");

    let _ = crate::palette::keys::handle_key(&mut app, ENTER, &mut effects);

    let state = app.palette().expect("open");
    assert_eq!(
        state.mode,
        PaletteMode::Name,
        "Enter must not enter Param mode on a refused row"
    );
    assert!(
        state.refusal.is_some(),
        "Enter on an unavailable row must set a refusal"
    );
}

#[test]
fn tab_accept_on_a_bare_command_fills_the_field_with_exactly_its_name() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    cursor_on(&mut app, "save");

    let _ = crate::palette::keys::handle_key(&mut app, TAB, &mut effects);

    let state = app.palette().expect("open");
    assert_eq!(state.field.text(), "save");
    assert_eq!(state.mode, PaletteMode::Name);
}

#[test]
fn tab_accept_on_a_parameterized_command_leaves_a_separator_before_the_argument() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    cursor_on(&mut app, "language");

    let _ = crate::palette::keys::handle_key(&mut app, TAB, &mut effects);
    let cmd_end = match app.palette().expect("open").mode {
        PaletteMode::Param { cmd_end, .. } => Some(cmd_end),
        PaletteMode::Name => None,
    }
    .expect("Tab on a parameterized command must enter Param mode");
    assert_eq!(
        &app.palette().expect("open").field.text()[..cmd_end],
        "language "
    );

    cursor_on_arg(&mut app, "markdown");
    let _ = crate::palette::keys::handle_key(&mut app, TAB, &mut effects);

    assert_eq!(
        app.palette().expect("open").field.text(),
        "language markdown",
        "the separator between the name and the argument must survive Tab-accept"
    );
}

#[test]
fn one_backspace_from_the_bare_parameterized_prefix_returns_to_name_mode() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    cursor_on(&mut app, "language");
    let _ = crate::palette::keys::handle_key(&mut app, TAB, &mut effects);
    assert!(matches!(
        app.palette().expect("open").mode,
        PaletteMode::Param { .. }
    ));
    assert_eq!(app.palette().expect("open").field.text(), "language ");

    let _ = crate::palette::keys::handle_key(&mut app, BACKSPACE, &mut effects);

    let state = app.palette().expect("open");
    assert_eq!(
        state.mode,
        PaletteMode::Name,
        "erasing back through the separator must return to Name mode"
    );
    assert_eq!(state.field.text(), "language");
}

const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};

/// Defect 4: ⌘V into the palette's query used to be swallowed silently —
/// neither `PALETTE_BINDINGS` nor `PasteTarget` had any entry for it.
#[test]
fn command_v_spawns_a_pbpaste_cmd_tagged_for_the_palette() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);

    let cmd_v = KeyInput {
        code: KeyCode::Char('v'),
        mods: SUP,
    };
    assert_eq!(
        crate::palette::keys::handle_key(&mut app, cmd_v, &mut effects),
        crate::keymap::KeyOutcome::Consumed
    );
    assert_eq!(
        effects.cmds.len(),
        1,
        "exactly one pbpaste read must be spawned"
    );
    assert!(app.palette().expect("open").field.is_empty());
}

#[test]
fn the_pbpaste_reply_for_the_palette_inserts_into_its_field() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);

    crate::app::update(
        &mut app,
        crate::runtime::Msg::ClipboardRead {
            text: "commit".to_string(),
            target: crate::runtime::PasteTarget::Palette,
        },
        &mut effects,
    );

    assert_eq!(app.palette().expect("open").field.text(), "commit");
}

/// Defect 4: Tab with nothing selected (an empty result list) used to be a
/// silent no-op — house rule, every user action gets feedback.
#[test]
fn tab_with_no_matching_command_reports_and_stays_open() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    if let Some(state) = app.palette_mut() {
        state.field.set_text("zzz_no_such_command_zzz");
    }
    recompute(&mut app);
    assert!(
        app.palette().expect("open").rows.is_empty(),
        "test setup: nothing matches"
    );

    let _ = crate::palette::keys::handle_key(&mut app, TAB, &mut effects);

    assert!(app.palette().is_some(), "an empty list must never close it");
    assert_eq!(
        app.palette().expect("open").refusal.as_deref(),
        Some("no matching command"),
        "Tab on an empty list must report feedback, not swallow the key"
    );
}
