use super::test_support::*;
use super::*;
use crate::keymap::KeyCode;

#[test]
fn ctrl_f_from_a_chip_returns_focus_to_the_find_field() {
    let mut app = app_with("hi");
    open_find(&mut app);
    press(&mut app, tab());
    assert_eq!(find(&app).focus, Control::Scope);
    open_find(&mut app);
    assert_eq!(find(&app).focus, Control::Find);
    assert!(app.find().is_some());
}

#[test]
fn tab_walks_the_ring_and_the_footer_names_the_focused_chip() {
    let mut app = app_with("hi");
    open_find(&mut app);
    let ring = [
        Control::Scope,
        Control::Case,
        Control::Word,
        Control::Regex,
        Control::Find,
    ];
    for expected in ring {
        press(&mut app, tab());
        assert_eq!(find(&app).focus, expected);
        if expected == Control::Word {
            let entries = crate::footer_hints::default_hint_entries(&app);
            assert_eq!(entries[0].1, "Word: off", "entries: {entries:?}");
            assert_eq!(entries[0].0, "\u{2423}");
        }
    }
    press(&mut app, shift_tab());
    assert_eq!(find(&app).focus, Control::Regex);
}

#[test]
fn space_on_the_case_chip_turns_it_on_and_drops_the_other_case() {
    let mut app = app_with("Dog dog");
    open_find(&mut app);
    type_str(&mut app, "dog");
    assert_eq!(find(&app).matches.len(), 2);

    press(&mut app, tab());
    press(&mut app, tab());
    assert_eq!(find(&app).focus, Control::Case);
    press(&mut app, space());

    assert!(find(&app).options.case_sensitive);
    assert_eq!(find(&app).matches, vec![4..7]);
    assert_eq!(selection_start(&app), 4);
}

#[test]
fn space_in_the_find_field_types_a_space() {
    let mut app = app_with("a b");
    open_find(&mut app);
    type_str(&mut app, "a");
    press(&mut app, space());
    type_str(&mut app, "b");
    assert_eq!(find(&app).find.draft, "a b");
    assert_eq!(find(&app).matches, vec![0..3]);
}

#[test]
fn alt_r_turns_regex_on_so_a_dot_matches_any_char() {
    let mut app = app_with("dog dig d.g");
    open_find(&mut app);
    type_str(&mut app, "d.g");
    assert_eq!(find(&app).matches, vec![8..11]);

    press(&mut app, key(KeyCode::Char('r'), ALT));
    assert!(find(&app).options.regex);
    assert_eq!(find(&app).matches.len(), 3);
    assert_eq!(selection_start(&app), 0);
}

#[test]
fn an_invalid_regex_shows_its_error_in_the_readout_and_paints_nothing() {
    let mut app = app_sized("(a) (b)", 120, 24);
    open_find(&mut app);
    press(&mut app, key(KeyCode::Char('r'), ALT));
    type_str(&mut app, "(");

    assert!(find(&app).pattern.is_err());
    assert!(find(&app).matches.is_empty());
    app.sync_view();
    let rows = crate::testgrid::grid(&app, 120, 24);
    let top = rows
        .iter()
        .position(|row| row.contains("\u{256d} Find"))
        .expect("panel on screen");
    assert!(
        rows[top + 1].contains("regex parse error"),
        "readout row: {:?}",
        rows[top + 1]
    );
    let buf = crate::testgrid::draw(&app, 120, 24);
    let painted = (0..120u16)
        .filter(|&x| {
            buf.cell((x, 2)).and_then(|c| c.style().bg) == app.theme.chrome.search_match_bg.bg
        })
        .count();
    assert_eq!(painted, 0);
}

#[test]
fn clicking_the_word_chip_toggles_whole_word() {
    let mut app = app_with("cat category");
    open_find(&mut app);
    type_str(&mut app, "cat");
    assert_eq!(find(&app).matches.len(), 2);

    let geo = crate::layout::geometry(app.frame_area(), &app);
    let (_, rect) = geo
        .find_panel
        .expect("panel open")
        .chips
        .iter()
        .flatten()
        .find(|(chip, _)| chip.control == Control::Word)
        .copied()
        .expect("the Word chip has a rect");
    click(&mut app, rect.x, rect.y);

    assert!(find(&app).options.whole_word);
    assert_eq!(find(&app).matches, vec![0..3]);
}

#[test]
fn clicking_into_the_editor_unfocuses_the_panel_and_dims_its_border() {
    let mut app = app_with("hello");
    open_find(&mut app);
    let geo = crate::layout::geometry(app.frame_area(), &app);
    let panel = geo.find_panel.expect("panel open");
    let active = app.theme.chrome.active_border.fg;
    let inactive = app.theme.chrome.inactive_border.fg;

    app.sync_view();
    let before = crate::testgrid::draw(&app, FRAME_W, FRAME_H);
    assert_eq!(
        before
            .cell((panel.frame.x, panel.frame.y))
            .and_then(|c| c.style().fg),
        active
    );

    click(&mut app, geo.editor.x, geo.editor.y);

    assert!(app.find().is_some(), "the panel is kept");
    assert!(!find(&app).focused);
    assert_eq!(
        crate::focus::target(&app),
        crate::focus::FocusTarget::Editor
    );
    let after = crate::testgrid::draw(&app, FRAME_W, FRAME_H);
    assert_eq!(
        after
            .cell((panel.frame.x, panel.frame.y))
            .and_then(|c| c.style().fg),
        inactive
    );
}

#[test]
fn clicking_inside_the_find_field_of_a_kept_panel_refocuses_it() {
    let mut app = app_with("hello");
    open_find(&mut app);
    crate::find::unfocus(&mut app);
    let panel = crate::layout::geometry(app.frame_area(), &app)
        .find_panel
        .expect("panel open");
    click(&mut app, panel.find_field.x, panel.find_field.y);
    assert!(find(&app).focused);
    assert_eq!(find(&app).focus, Control::Find);
}

#[test]
fn ctrl_r_expands_the_panel_to_five_rows_with_a_replace_rule() {
    let mut app = app_with("hello");
    open_find(&mut app);
    open_replace(&mut app);

    assert!(find(&app).replace.is_some());
    assert_eq!(find(&app).focus, Control::Replace);
    let rows = grid(&mut app);
    let top = usize::from(FRAME_H) - 6;
    assert!(rows[top].contains("\u{256d} Find"), "top: {:?}", rows[top]);
    assert!(
        rows[top + 2].starts_with("\u{251c} Replace"),
        "rule: {:?}",
        rows[top + 2]
    );
    assert!(
        rows[top + 3].contains("[Replace] [All]"),
        "chips: {:?}",
        rows[top + 3]
    );
    assert!(
        rows[top + 4].starts_with('\u{2570}'),
        "bottom: {:?}",
        rows[top + 4]
    );

    open_replace(&mut app);
    assert!(app.find().is_some(), "^R never closes the panel");
}

#[test]
fn ctrl_r_on_a_closed_panel_opens_it_with_the_replace_field_focused() {
    let mut app = app_with("hello");
    open_replace(&mut app);
    assert_eq!(find(&app).focus, Control::Replace);
    assert_eq!(find(&app).control_ring().len(), 8);
}

#[test]
fn the_footer_inside_the_panel_lists_only_panel_keys() {
    let mut app = app_with("hello");
    open_find(&mut app);
    let entries = crate::footer_hints::default_hint_entries(&app);
    let helps: Vec<&str> = entries.iter().map(|(_, help, _)| help.as_ref()).collect();
    assert!(!helps.contains(&"save"), "{helps:?}");
    assert!(!helps.contains(&"quit"), "{helps:?}");
    assert!(helps.contains(&"next match"), "{helps:?}");
    assert!(helps.contains(&"replace"), "{helps:?}");
    assert_eq!(
        entries.first().map(|(label, _, _)| label.as_str()),
        Some("\u{238b}"),
        "the closing key leads so width truncation never drops it"
    );
    let text = crate::footer::footer_text(&app);
    assert!(text.contains("\u{23ce} next match"), "{text:?}");
}
