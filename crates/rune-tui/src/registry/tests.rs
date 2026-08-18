#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;

use crate::diff_view::keys::DIFF_BINDINGS;
use crate::explorer_keys::EXPLORER_BINDINGS;
use crate::explorer_search::EXPLORER_SEARCH_BINDINGS;
use crate::filesearch::keys::FILESEARCH_BINDINGS;
use crate::global::GLOBAL_BINDINGS;
use crate::keymap::editor_bindings::EDITOR_BINDINGS;
use crate::opentabs::TABS_BINDINGS;

use super::rows;
use super::*;

fn row_count_for(id: CommandId) -> usize {
    rows::registry().iter().filter(|row| row.id == id).count()
}

#[test]
fn every_global_binding_maps_to_exactly_one_registry_row() {
    for binding in GLOBAL_BINDINGS {
        let id = rows::global::adapt(binding.cmd);
        assert_eq!(
            row_count_for(id),
            1,
            "no unique registry row for global binding {:?}",
            binding.help
        );
    }
}

#[test]
fn every_editor_binding_maps_to_exactly_one_registry_row() {
    for binding in EDITOR_BINDINGS {
        let id = rows::editor::adapt(binding.cmd);
        assert_eq!(
            row_count_for(id),
            1,
            "no unique registry row for editor binding {:?}",
            binding.help
        );
    }
}

#[test]
fn every_explorer_binding_maps_to_exactly_one_registry_row() {
    for binding in EXPLORER_BINDINGS {
        let id = rows::pane::adapt_explorer(binding.cmd);
        assert_eq!(row_count_for(id), 1, "explorer binding {:?}", binding.help);
    }
}

#[test]
fn every_explorer_search_binding_maps_to_exactly_one_registry_row() {
    for binding in EXPLORER_SEARCH_BINDINGS {
        let id = rows::pane::adapt_explorer_search(binding.cmd);
        assert_eq!(
            row_count_for(id),
            1,
            "explorer search binding {:?}",
            binding.help
        );
    }
}

#[test]
fn every_tabs_binding_maps_to_exactly_one_registry_row() {
    for binding in TABS_BINDINGS {
        let id = rows::pane::adapt_tabs(binding.cmd);
        assert_eq!(row_count_for(id), 1, "tabs binding {:?}", binding.help);
    }
}

#[test]
fn every_filesearch_binding_maps_to_exactly_one_registry_row() {
    for binding in FILESEARCH_BINDINGS {
        let id = rows::pane::adapt_filesearch(binding.cmd);
        assert_eq!(
            row_count_for(id),
            1,
            "filesearch binding {:?}",
            binding.help
        );
    }
}

#[test]
fn every_diff_binding_maps_to_exactly_one_registry_row() {
    for binding in DIFF_BINDINGS {
        let id = rows::pane::adapt_diff(binding.cmd);
        assert_eq!(row_count_for(id), 1, "diff binding {:?}", binding.help);
    }
}

#[test]
fn every_listed_row_name_is_unique() {
    let mut seen = HashSet::new();
    for row in rows::registry().iter().filter(|row| row.listed) {
        assert!(
            seen.insert(row.name),
            "listed name {:?} is not unique",
            row.name
        );
    }
}

#[test]
fn every_fuzzy_alias_resolves_uniquely() {
    let listed_names: HashSet<&str> = rows::registry()
        .iter()
        .filter(|row| row.listed)
        .map(|row| row.name)
        .collect();
    let mut seen_aliases = HashSet::new();
    for row in rows::registry() {
        for alias in row.fuzzy_aliases {
            assert!(
                !listed_names.contains(alias),
                "alias {alias:?} collides with a listed command name"
            );
            assert!(
                seen_aliases.insert(*alias),
                "alias {alias:?} is claimed by more than one row"
            );
        }
    }
    assert!(
        !seen_aliases.is_empty(),
        "no row declares a fuzzy alias, so this guard cannot fire on a real collision"
    );
}

#[test]
fn chords_round_trip_for_move_line_up() {
    let id = CommandId::Editor(crate::keymap::Command::MoveLineUp);
    let via_chords: Vec<_> = chords(id).collect();
    let via_table: Vec<_> = EDITOR_BINDINGS
        .iter()
        .filter(|b| rows::editor::adapt(b.cmd) == id)
        .map(|b| b.key)
        .collect();
    assert_eq!(via_chords, via_table);
    assert_eq!(via_chords.len(), 1);
}

#[test]
fn chords_round_trip_for_toggle_explorer() {
    let id = CommandId::Global(crate::global::GlobalCommand::ToggleLeft);
    let via_chords: Vec<_> = chords(id).collect();
    let via_table: Vec<_> = GLOBAL_BINDINGS
        .iter()
        .filter(|b| rows::global::adapt(b.cmd) == id)
        .map(|b| b.key)
        .collect();
    assert_eq!(via_chords, via_table);
    assert_eq!(via_chords.len(), 2);
}

#[test]
fn chords_round_trip_collapses_both_quit_chords_into_one_row() {
    let id = CommandId::Global(crate::global::GlobalCommand::QuitChord(
        crate::keymap::QuitKey::CtrlC,
    ));
    let via_chords: Vec<_> = chords(id).collect();
    assert_eq!(via_chords.len(), 2);
}

#[test]
fn chords_round_trip_collapses_every_tab_switch_into_the_tab_command() {
    let id = CommandId::Palette(PaletteCommand::TabByName);
    let via_chords: Vec<_> = chords(id).collect();
    assert_eq!(via_chords.len(), 10);
}

#[test]
fn spec_returns_the_row_matching_its_id() {
    let id = CommandId::Editor(crate::keymap::Command::Save);
    let found = spec(id).expect("save must have a registry row");
    assert_eq!(found.id, id);
    assert_eq!(found.name, "save");
}
