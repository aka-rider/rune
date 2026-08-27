use super::*;

#[test]
fn ctrl_m_resolves_to_merge_and_sup_m_binds_nothing() {
    use crate::binding::resolve_in;
    use crate::keymap::KeyInput;

    let ctrl_m = KeyInput {
        code: KeyCode::Char('m'),
        mods: CTRL,
    };
    let sup_m = KeyInput {
        code: KeyCode::Char('m'),
        mods: SUP,
    };

    assert_eq!(
        resolve_in(GLOBAL_BINDINGS, ctrl_m),
        Some(GlobalCommand::Merge)
    );
    assert_eq!(resolve_in(GLOBAL_BINDINGS, sup_m), None);
    assert!(
        GLOBAL_BINDINGS.iter().all(|b| !b.key.matches(sup_m)),
        "no row should match sup+m"
    );
}

#[test]
fn every_printable_binding_requires_a_modifier() {
    use crate::binding::KeyMatch;
    for binding in GLOBAL_BINDINGS {
        let key = binding.key;
        if !matches!(key.key, KeyMatch::Code(KeyCode::Char(_))) {
            continue;
        }
        assert!(
            key.mods.ctrl || key.mods.sup,
            "{:?} has no ctrl/sup modifier and could shadow text input",
            key
        );
    }
}

fn claimants<C: Copy + 'static>(
    table: &[Binding<C>],
    key: crate::keymap::KeyInput,
) -> Vec<&'static str> {
    table
        .iter()
        .filter(|b| b.key.matches(key))
        .map(|b| b.help)
        .collect()
}

fn claimants_across_established_pane_tables(key: crate::keymap::KeyInput) -> Vec<&'static str> {
    use crate::explorer_keys::EXPLORER_BINDINGS;
    use crate::explorer_search::EXPLORER_SEARCH_BINDINGS;
    use crate::filesearch::keys::FILESEARCH_BINDINGS;
    use crate::keymap::editor_bindings::EDITOR_BINDINGS;
    use crate::keymap::vim::VIM_BINDINGS;
    use crate::opentabs::TABS_BINDINGS;

    [
        claimants(EDITOR_BINDINGS, key),
        claimants(VIM_BINDINGS, key),
        claimants(TABS_BINDINGS, key),
        claimants(EXPLORER_BINDINGS, key),
        claimants(EXPLORER_SEARCH_BINDINGS, key),
        claimants(FILESEARCH_BINDINGS, key),
    ]
    .concat()
}

fn claimants_across_pane_tables(key: crate::keymap::KeyInput) -> Vec<&'static str> {
    use crate::diff_view::keys::DIFF_BINDINGS;

    [
        claimants_across_established_pane_tables(key),
        claimants(DIFF_BINDINGS, key),
    ]
    .concat()
}

fn assert_unclaimed_by_any_pane_table(keys: &[crate::keymap::KeyInput]) {
    for key in keys {
        let found = claimants_across_pane_tables(*key);
        assert!(
            found.is_empty(),
            "{key:?} is already bound in a pane table: {found:?}"
        );
    }
}

#[test]
fn diff_bindings_are_unclaimed_by_the_global_table_and_every_pane_table() {
    use crate::binding::KeyMatch;
    use crate::diff_view::keys::DIFF_BINDINGS;
    use crate::keymap::KeyInput;

    for binding in DIFF_BINDINGS {
        let pattern = binding.key;
        let key = match pattern.key {
            KeyMatch::Code(code) => KeyInput {
                code,
                mods: pattern.mods,
            },
            KeyMatch::Printable => continue,
        };
        let global_claimants: Vec<&'static str> = GLOBAL_BINDINGS
            .iter()
            .filter(|b| b.key.matches(key))
            .map(|b| b.help)
            .collect();
        assert!(
            global_claimants.is_empty(),
            "GLOBAL_BINDINGS would shadow diff key {key:?}: {global_claimants:?}"
        );
        let pane_claimants = claimants_across_established_pane_tables(key);
        assert!(
            pane_claimants.is_empty(),
            "a pane table would shadow diff key {key:?}: {pane_claimants:?}"
        );
    }
}

#[test]
fn global_p_binding_is_not_already_bound_in_any_pane_table() {
    use crate::keymap::KeyInput;

    let ctrl_p = KeyInput {
        code: KeyCode::Char('p'),
        mods: CTRL,
    };
    let sup_p = KeyInput {
        code: KeyCode::Char('p'),
        mods: SUP,
    };
    assert_unclaimed_by_any_pane_table(&[ctrl_p, sup_p]);
}

#[test]
fn global_m_binding_is_not_already_bound_in_any_pane_table() {
    use crate::keymap::KeyInput;

    let ctrl_m = KeyInput {
        code: KeyCode::Char('m'),
        mods: CTRL,
    };
    assert_unclaimed_by_any_pane_table(&[ctrl_m]);
}

#[test]
fn global_s_binding_is_not_already_bound_in_any_pane_table() {
    use crate::keymap::KeyInput;

    let ctrl_s = KeyInput {
        code: KeyCode::Char('s'),
        mods: CTRL,
    };
    assert_unclaimed_by_any_pane_table(&[ctrl_s]);
}

#[test]
fn save_has_a_canonical_row_labelled_ctrl_s() {
    assert_eq!(
        hint_for(GlobalCommand::Save),
        Some(("^S".to_string(), "save"))
    );
}

#[test]
fn global_e_binding_is_not_already_bound_in_any_pane_table() {
    use crate::keymap::KeyInput;

    let ctrl_e = KeyInput {
        code: KeyCode::Char('e'),
        mods: CTRL,
    };
    let sup_e = KeyInput {
        code: KeyCode::Char('e'),
        mods: SUP,
    };
    assert_unclaimed_by_any_pane_table(&[ctrl_e, sup_e]);
}

#[test]
fn global_f_binding_is_not_already_bound_in_any_pane_table() {
    use crate::keymap::KeyInput;

    let ctrl_f = KeyInput {
        code: KeyCode::Char('f'),
        mods: CTRL,
    };
    let sup_f = KeyInput {
        code: KeyCode::Char('f'),
        mods: SUP,
    };
    assert_unclaimed_by_any_pane_table(&[ctrl_f, sup_f]);
}

#[test]
fn global_g_binding_is_not_already_bound_in_any_pane_table() {
    use crate::keymap::KeyInput;

    let ctrl_g = KeyInput {
        code: KeyCode::Char('g'),
        mods: CTRL,
    };
    let sup_g = KeyInput {
        code: KeyCode::Char('g'),
        mods: SUP,
    };
    let ctrl_cap_g = KeyInput {
        code: KeyCode::Char('G'),
        mods: CTRL,
    };
    let sup_cap_g = KeyInput {
        code: KeyCode::Char('G'),
        mods: SUP,
    };
    assert_unclaimed_by_any_pane_table(&[ctrl_g, sup_g, ctrl_cap_g, sup_cap_g]);
}

#[test]
fn ctrl_shifted_g_resolves_to_search_prev() {
    use crate::binding::resolve_in;
    use crate::keymap::KeyInput;

    let ctrl_cap_g = KeyInput {
        code: KeyCode::Char('G'),
        mods: CTRL,
    };
    assert_eq!(
        resolve_in(GLOBAL_BINDINGS, ctrl_cap_g),
        Some(GlobalCommand::SearchPrev)
    );
}

#[test]
fn global_n_binding_is_not_already_bound_in_any_pane_table() {
    use crate::keymap::KeyInput;

    let ctrl_n = KeyInput {
        code: KeyCode::Char('n'),
        mods: CTRL,
    };
    let sup_n = KeyInput {
        code: KeyCode::Char('n'),
        mods: SUP,
    };
    assert_unclaimed_by_any_pane_table(&[ctrl_n, sup_n]);
}

/// Trash is no longer a global chord (product decision): neither `⌘⌫` nor
/// `^⌫` resolves through `GLOBAL_BINDINGS` any more, in the editor, the
/// title field, the finder, or anywhere else a raw key dispatch checks this
/// table first.
#[test]
fn global_bindings_no_longer_claim_backspace_at_all() {
    use crate::binding::resolve_in;
    use crate::keymap::KeyInput;

    let sup_backspace = KeyInput {
        code: KeyCode::Backspace,
        mods: SUP,
    };
    let ctrl_backspace = KeyInput {
        code: KeyCode::Backspace,
        mods: CTRL,
    };

    for key in [sup_backspace, ctrl_backspace] {
        assert_eq!(
            resolve_in(GLOBAL_BINDINGS, key),
            None,
            "{key:?} must not resolve through the global table any more"
        );
    }
}

/// Trash's new home: an Explorer-pane-scoped binding on `⌘⌫` and the
/// forward-delete key — `^⌫` deliberately does NOT survive the move, only
/// the two chords the product decision named.
#[test]
fn explorer_owns_sup_backspace_and_delete_for_trash() {
    use crate::binding::resolve_in;
    use crate::explorer_keys::{EXPLORER_BINDINGS, ExplorerCommand};
    use crate::keymap::KeyInput;

    let sup_backspace = KeyInput {
        code: KeyCode::Backspace,
        mods: SUP,
    };
    let ctrl_backspace = KeyInput {
        code: KeyCode::Backspace,
        mods: CTRL,
    };
    let delete = KeyInput {
        code: KeyCode::Delete,
        mods: Mods::NONE,
    };

    assert_eq!(
        resolve_in(EXPLORER_BINDINGS, sup_backspace),
        Some(ExplorerCommand::Trash)
    );
    assert_eq!(
        resolve_in(EXPLORER_BINDINGS, delete),
        Some(ExplorerCommand::Trash)
    );
    assert_eq!(
        resolve_in(EXPLORER_BINDINGS, ctrl_backspace),
        None,
        "^⌫ must not survive the move to the Explorer pane"
    );
}

#[test]
fn global_j_binding_is_not_already_bound_in_any_pane_table() {
    use crate::keymap::KeyInput;

    let ctrl_j = KeyInput {
        code: KeyCode::Char('j'),
        mods: CTRL,
    };
    assert_unclaimed_by_any_pane_table(&[ctrl_j]);
}

#[test]
fn filesearch_chords_are_not_already_bound_in_any_pane_table() {
    use crate::keymap::KeyInput;

    let ctrl_cap_f = KeyInput {
        code: KeyCode::Char('F'),
        mods: CTRL,
    };
    let sup_cap_f = KeyInput {
        code: KeyCode::Char('F'),
        mods: SUP,
    };
    assert_unclaimed_by_any_pane_table(&[ctrl_cap_f, sup_cap_f]);
}

#[test]
fn ctrl_shifted_f_resolves_to_toggle_filesearch() {
    use crate::binding::resolve_in;
    use crate::keymap::KeyInput;

    let ctrl_cap_f = KeyInput {
        code: KeyCode::Char('F'),
        mods: CTRL,
    };
    assert_eq!(
        resolve_in(GLOBAL_BINDINGS, ctrl_cap_f),
        Some(GlobalCommand::ToggleFileSearch)
    );
}

#[test]
fn palette_chords_are_not_already_bound_in_any_pane_table() {
    use crate::keymap::KeyInput;

    let ctrl_cap_p = KeyInput {
        code: KeyCode::Char('P'),
        mods: CTRL,
    };
    let sup_cap_p = KeyInput {
        code: KeyCode::Char('P'),
        mods: SUP,
    };
    assert_unclaimed_by_any_pane_table(&[ctrl_cap_p, sup_cap_p]);
}

#[test]
fn ctrl_shifted_p_resolves_to_toggle_palette() {
    use crate::binding::resolve_in;
    use crate::keymap::KeyInput;

    let ctrl_cap_p = KeyInput {
        code: KeyCode::Char('P'),
        mods: CTRL,
    };
    assert_eq!(
        resolve_in(GLOBAL_BINDINGS, ctrl_cap_p),
        Some(GlobalCommand::TogglePalette)
    );
}

#[test]
fn ctrl_shifted_p_reaches_toggle_palette_through_from_termina() {
    use termina::event::{KeyCode as TerminaKeyCode, KeyEvent, Modifiers};

    let event = KeyEvent::new(TerminaKeyCode::Char('P'), Modifiers::CONTROL);
    let input = crate::keymap::from_termina(event);
    assert_eq!(
        input.and_then(|key| crate::binding::resolve_in(GLOBAL_BINDINGS, key)),
        Some(GlobalCommand::TogglePalette)
    );
}
