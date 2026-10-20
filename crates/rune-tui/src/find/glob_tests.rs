use super::test_support::*;
use super::*;
use crate::keymap::{KeyCode, KeyInput};
use crate::runtime::{CmdKind, Effects};

const TREE: [(&str, &[u8]); 6] = [
    ("/root/a.md", b"needle"),
    ("/root/docs/b.md", b"needle"),
    ("/root/docs/deep/c.md", b"needle"),
    ("/root/src/d.rs", b"needle"),
    ("/root/a/e.md", b"needle"),
    ("/root/b/a/f.md", b"needle"),
];

fn alt_f() -> KeyInput {
    key(KeyCode::Char('f'), ALT)
}

fn queries_dispatched(effects: &Effects) -> usize {
    effects
        .cmds
        .iter()
        .filter(|cmd| cmd.kind() == CmdKind::ProjectQuery)
        .count()
}

fn workspace_with_query(files: &[(&str, &[u8])]) -> (App, Effects) {
    let mut app = seeded_app(files);
    let mut effects = open_project_find(&mut app);
    pump_index(&mut app, &mut effects);
    type_into(&mut app, "needle", &mut effects);
    (app, effects)
}

fn fill_globs(app: &mut App, include: &str, exclude: &str, effects: &mut Effects) {
    press_into(app, tab(), effects);
    assert_eq!(find(app).focus, Control::Include);
    type_into(app, include, effects);
    press_into(app, tab(), effects);
    assert_eq!(find(app).focus, Control::Exclude);
    type_into(app, exclude, effects);
}

fn searched_with_globs(files: &[(&str, &[u8])], include: &str, exclude: &str) -> Vec<String> {
    let (mut app, mut effects) = workspace_with_query(files);
    fill_globs(&mut app, include, exclude, &mut effects);
    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);
    let mut displays = result_displays(&app);
    displays.sort();
    displays
}

#[test]
fn include_md_with_exclude_docs_returns_no_hit_under_docs() {
    let found = searched_with_globs(&TREE, "*.md", "docs/**");

    assert_eq!(found, vec!["a.md", "a/e.md", "b/a/f.md"]);
}

#[test]
fn a_pattern_without_a_slash_matches_nested_files() {
    let found = searched_with_globs(&TREE, "*.md", "");

    assert_eq!(
        found,
        vec!["a.md", "a/e.md", "b/a/f.md", "docs/b.md", "docs/deep/c.md"]
    );
}

#[test]
fn a_pattern_with_a_slash_is_anchored_at_the_workspace_root() {
    let found = searched_with_globs(&TREE, "a/*.md", "");

    assert_eq!(found, vec!["a/e.md"]);
}

#[test]
fn a_bare_directory_name_excludes_everything_beneath_it() {
    let found = searched_with_globs(&TREE, "", "docs");

    assert_eq!(found, vec!["a.md", "a/e.md", "b/a/f.md", "src/d.rs"]);
}

#[test]
fn several_comma_separated_entries_with_spaces_are_alternatives() {
    let found = searched_with_globs(&TREE, " *.rs , a/*.md ", "");

    assert_eq!(found, vec!["a/e.md", "src/d.rs"]);
}

#[test]
fn empty_whitespace_and_comma_only_fields_filter_nothing() {
    for text in ["", "   ", ",,", " , , "] {
        let found = searched_with_globs(&TREE, text, text);

        assert_eq!(found.len(), TREE.len(), "{text:?}");
    }
}

#[test]
fn an_invalid_glob_shows_the_error_and_queries_nothing() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    fill_globs(&mut app, "[", "", &mut effects);

    fire_debounce(&mut app, &mut effects);

    assert_eq!(queries_dispatched(&effects), 0);
    let readout = project_readout_row(&mut app);
    assert!(readout.contains("include: "), "{readout:?}");
    assert!(project(&app).results.is_empty());
}

#[test]
fn an_invalid_glob_discards_a_reply_still_in_flight() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    fire_debounce(&mut app, &mut effects);
    let in_flight = run_one_cmd(&mut effects, CmdKind::ProjectQuery).expect("query dispatched");
    fill_globs(&mut app, "[", "", &mut effects);
    fire_debounce(&mut app, &mut effects);

    crate::app::update(&mut app, in_flight, &mut effects);

    assert!(project(&app).results.is_empty());
}

#[test]
fn fixing_an_invalid_glob_brings_the_results_back() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    fill_globs(&mut app, "[", "", &mut effects);
    fire_debounce(&mut app, &mut effects);

    press_into(&mut app, shift_tab(), &mut effects);
    press_into(&mut app, backspace(), &mut effects);
    type_into(&mut app, "*.rs", &mut effects);
    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);

    assert_eq!(result_displays(&app), vec!["src/d.rs"]);
}

#[test]
fn typing_into_include_requeries_after_the_debounce() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);
    assert_eq!(result_displays(&app).len(), TREE.len());

    press_into(&mut app, tab(), &mut effects);
    type_into(&mut app, "*.rs", &mut effects);
    assert_eq!(
        queries_dispatched(&effects),
        0,
        "keystrokes only arm the timer"
    );
    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);

    assert_eq!(result_displays(&app), vec!["src/d.rs"]);
}

#[test]
fn an_unsaved_edit_in_an_excluded_file_is_not_searched() {
    let mut app = seeded_app(&[("/root/docs/a.md", b"plain"), ("/root/b.md", b"plain")]);
    let mut effects = Effects::default();
    crate::workspace::open_path_checked(
        &mut app,
        std::path::Path::new("/root/docs/a.md"),
        &mut effects,
    )
    .expect("open the disk file");
    type_into(&mut app, "needle", &mut effects);
    press_into(&mut app, key(KeyCode::Char('F'), CTRL), &mut effects);
    pump_index(&mut app, &mut effects);
    type_into(&mut app, "needle", &mut effects);
    fill_globs(&mut app, "", "docs/", &mut effects);

    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);

    assert!(result_displays(&app).is_empty());
}

#[test]
fn tab_visits_include_and_exclude_only_in_the_workspace_filter() {
    let mut app = seeded_app(&TREE);
    let mut effects = Effects::default();
    press_into(&mut app, key(KeyCode::Char('f'), CTRL), &mut effects);
    open_replace(&mut app);
    assert_eq!(
        find(&app).control_ring(),
        vec![Control::Find, Control::Replace]
    );

    press_into(&mut app, alt_f(), &mut effects);

    assert_eq!(
        find(&app).control_ring(),
        vec![
            Control::Find,
            Control::Replace,
            Control::Include,
            Control::Exclude
        ]
    );
    press_into(&mut app, tab(), &mut effects);
    assert_eq!(find(&app).focus, Control::Include);
    press_into(&mut app, tab(), &mut effects);
    assert_eq!(find(&app).focus, Control::Exclude);
    press_into(&mut app, tab(), &mut effects);
    assert_eq!(find(&app).focus, Control::Find, "wraps past the last field");
}

#[test]
fn switching_back_to_this_file_moves_focus_off_the_glob_fields() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    press_into(&mut app, tab(), &mut effects);
    assert_eq!(find(&app).focus, Control::Include);

    press_into(&mut app, alt_f(), &mut effects);

    assert_eq!(find(&app).filter_kind(), FilterKind::ThisFile);
    assert_eq!(find(&app).focus, Control::Find);
}

#[test]
fn the_glob_row_is_drawn_only_in_the_workspace_filter() {
    let mut app = seeded_app(&TREE);
    let mut effects = Effects::default();
    press_into(&mut app, key(KeyCode::Char('f'), CTRL), &mut effects);
    assert!(
        !project_grid(&mut app)
            .iter()
            .any(|row| row.contains("include "))
    );

    press_into(&mut app, alt_f(), &mut effects);

    let rows = project_grid(&mut app);
    let row = rows
        .iter()
        .find(|row| row.contains("include "))
        .expect("the glob row is on screen");
    assert!(row.contains("exclude "), "{row:?}");
}

#[test]
fn clicking_a_glob_field_focuses_it_and_typing_lands_there() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    let panel = crate::layout::geometry(app.frame_area(), &app)
        .find_panel
        .expect("panel open");
    let exclude = panel.exclude_field.expect("the exclude field has a rect");

    click(&mut app, exclude.x + 1, exclude.y);
    type_into(&mut app, "src", &mut effects);

    assert_eq!(find(&app).focus, Control::Exclude);
    let globs = find(&app).globs().expect("workspace filter");
    assert_eq!(globs.exclude.editor.text(), "src");
    assert_eq!(globs.include.editor.text(), "");
}

#[test]
fn pasting_lands_in_the_focused_glob_field() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    press_into(&mut app, tab(), &mut effects);

    super::keys::paste(&mut app, "*.md");

    let globs = find(&app).globs().expect("workspace filter");
    assert_eq!(globs.include.editor.text(), "*.md");
    assert_eq!(find(&app).find.editor.text(), "needle");
}

#[test]
fn up_and_down_in_a_glob_field_say_there_is_no_history() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    press_into(&mut app, tab(), &mut effects);

    for arrow in [up(), down()] {
        press_into(&mut app, arrow, &mut effects);
        let text = crate::messages::newest_text(&app).unwrap_or_default();
        assert!(text.contains("no history"), "{text:?}");
    }
}

#[test]
fn the_footer_names_the_next_field_from_a_glob_field() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    press_into(&mut app, tab(), &mut effects);

    let helps = |app: &App| -> Vec<String> {
        hints::entries(app)
            .into_iter()
            .map(|(_, help, _)| help.into_owned())
            .collect()
    };
    assert!(helps(&app).contains(&"exclude".to_string()));
    press_into(&mut app, tab(), &mut effects);
    assert!(helps(&app).contains(&"find field".to_string()));
}

#[test]
fn include_and_exclude_text_survives_toggling_this_file_on_and_off() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    fill_globs(&mut app, "*.md", "docs/**", &mut effects);

    press_into(&mut app, alt_f(), &mut effects);
    assert_eq!(find(&app).filter_kind(), FilterKind::ThisFile);
    assert!(find(&app).globs().is_none(), "the fields are hidden");
    assert!(!find(&app).control_ring().contains(&Control::Include));
    assert!(!find(&app).control_ring().contains(&Control::Exclude));
    press_into(&mut app, alt_f(), &mut effects);

    let globs = find(&app).globs().expect("workspace filter");
    assert_eq!(globs.include.editor.text(), "*.md");
    assert_eq!(globs.exclude.editor.text(), "docs/**");
    pump_index(&mut app, &mut effects);
    fire_debounce(&mut app, &mut effects);
    while let Some(reply) = run_one_cmd(&mut effects, CmdKind::ProjectQuery) {
        crate::app::update(&mut app, reply, &mut effects);
    }
    let mut displays = result_displays(&app);
    displays.sort();
    assert_eq!(displays, vec!["a.md", "a/e.md", "b/a/f.md"]);
}

#[test]
fn drawing_frames_does_not_recompile_the_globs() {
    let (mut app, mut effects) = workspace_with_query(&TREE);
    fill_globs(&mut app, "*.md", "docs/**", &mut effects);
    project_grid(&mut app);
    let compiled = globs::compilations();

    for _ in 0..3 {
        project_grid(&mut app);
    }

    assert_eq!(globs::compilations(), compiled);
}
