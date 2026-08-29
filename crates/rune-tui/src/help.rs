use crate::binding::{KeyMatch, KeyPattern};
use crate::registry::{self, CommandId, CommandSpec};

// This module stays termina-free, so the caller does the keyboard-flags
// probe and hands over a plain bool. `false` means the probe came back
// missing a bit this app asked for, so every `⌘` chord below gets an
// `\u{26a0}` marker rather than guessing which specific chord the
// terminal actually meant.
pub fn help_markdown(sup_chords_reliable: bool) -> String {
    let mut out = String::from("# Help\n\n");
    if !sup_chords_reliable {
        out.push_str(
            "_this terminal never confirmed key disambiguation \u{2014} rows marked \u{26a0} may not reach rune here._\n\n",
        );
    }
    push_section(&mut out, "Global", is_global, sup_chords_reliable);
    push_section(&mut out, "Explorer", is_explorer, sup_chords_reliable);
    push_section(&mut out, "Open File", is_file_search, sup_chords_reliable);
    push_section(&mut out, "Open Tabs", is_open_tabs, sup_chords_reliable);
    push_section(&mut out, "Editor", is_editor, sup_chords_reliable);
    push_section(&mut out, "Diff View", is_diff, sup_chords_reliable);
    push_section(&mut out, "Palette", is_palette, sup_chords_reliable);
    push_section(
        &mut out,
        "Palette Keys",
        is_palette_key,
        sup_chords_reliable,
    );
    out
}

fn is_global(id: CommandId) -> bool {
    matches!(id, CommandId::Global(_))
}

fn is_explorer(id: CommandId) -> bool {
    matches!(id, CommandId::Explorer(_) | CommandId::ExplorerSearch(_))
}

fn is_file_search(id: CommandId) -> bool {
    matches!(id, CommandId::FileSearch(_))
}

fn is_open_tabs(id: CommandId) -> bool {
    matches!(id, CommandId::Tabs(_))
}

fn is_editor(id: CommandId) -> bool {
    matches!(id, CommandId::Editor(_))
}

fn is_diff(id: CommandId) -> bool {
    matches!(id, CommandId::Diff(_))
}

fn is_palette(id: CommandId) -> bool {
    matches!(id, CommandId::Palette(_))
}

fn is_palette_key(id: CommandId) -> bool {
    matches!(id, CommandId::PaletteKey(_))
}

fn push_section(
    out: &mut String,
    title: &str,
    pick: fn(CommandId) -> bool,
    sup_chords_reliable: bool,
) {
    out.push_str("## ");
    out.push_str(title);
    out.push_str("\n\n| Command | Key, Alt Key |\n| --- | --- |\n");
    for row in registry::rows::registry().iter().filter(|row| pick(row.id)) {
        push_command_row(out, row, sup_chords_reliable);
    }
    out.push('\n');
}

fn push_command_row(out: &mut String, row: &CommandSpec, sup_chords_reliable: bool) {
    let chords: Vec<KeyPattern> = registry::chords(row.id).collect();
    let has_chords = !chords.is_empty();
    let mut labels: Vec<String> = Vec::new();
    for chord in chords
        .into_iter()
        .filter(|chord| chord.key != KeyMatch::Printable)
    {
        let mut label = chord.label();
        if chord.mods.sup && !sup_chords_reliable {
            label.push_str(" \u{26a0}");
        }
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    if !row.listed && !has_chords {
        return;
    }
    let key = if labels.is_empty() {
        "\u{2014}".to_string()
    } else {
        labels.join(", ")
    };
    let name = if row.detail.is_empty() {
        row.name.to_string()
    } else {
        format!("{} \u{2014} {}", row.name, row.detail)
    };
    push_row(out, &name, &key);
}

fn push_row(out: &mut String, name: &str, key: &str) {
    out.push_str("| ");
    out.push_str(name);
    out.push_str(" | ");
    out.push_str(key);
    out.push_str(" |\n");
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::global::GLOBAL_BINDINGS;
    use crate::global::GlobalCommand;
    use crate::keymap::Command;
    use crate::keymap::editor_bindings::{EDITOR_BINDINGS, RELOAD};
    use crate::palette::keys::PaletteKeyCommand;
    use crate::registry::PaletteCommand;

    fn section_of<'a>(md: &'a str, title: &str) -> &'a str {
        let heading = format!("## {title}\n\n");
        let after = md
            .split(heading.as_str())
            .nth(1)
            .expect("section heading present");
        after.split("\n## ").next().expect("at least one piece")
    }

    fn row_count(section: &str) -> usize {
        section
            .lines()
            .filter(|line| {
                line.starts_with("| ")
                    && *line != "| Command | Key, Alt Key |"
                    && *line != "| --- | --- |"
            })
            .count()
    }

    fn expected_row_count(pick: fn(CommandId) -> bool) -> usize {
        registry::rows::registry()
            .iter()
            .filter(|row| pick(row.id))
            .filter(|row| row.listed || registry::chords(row.id).count() > 0)
            .count()
    }

    #[test]
    fn generates_headings_for_every_section() {
        let md = help_markdown(true);
        assert!(md.starts_with("# Help\n"));
        for heading in [
            "## Global",
            "## Explorer",
            "## Open File",
            "## Open Tabs",
            "## Editor",
            "## Diff View",
            "## Palette",
            "## Palette Keys",
        ] {
            assert!(md.contains(heading), "missing {heading:?} in:\n{md}");
        }
    }

    #[test]
    fn the_explorer_section_is_one_table_fed_by_both_binding_sets() {
        let md = help_markdown(true);
        assert_eq!(md.matches("## Explorer").count(), 1);
        let section = section_of(&md, "Explorer");
        assert_eq!(row_count(section), expected_row_count(is_explorer));
    }

    #[test]
    fn every_included_row_name_detail_and_chord_labels_appear() {
        let md = help_markdown(true);
        for row in registry::rows::registry() {
            let chords: Vec<KeyPattern> = registry::chords(row.id).collect();
            if chords.is_empty() && !row.listed {
                continue;
            }
            assert!(md.contains(row.name), "missing name {:?}", row.name);
            if !row.detail.is_empty() {
                assert!(md.contains(row.detail), "missing detail {:?}", row.detail);
            }
            for chord in &chords {
                if chord.key == KeyMatch::Printable {
                    continue;
                }
                assert!(
                    md.contains(&chord.label()),
                    "missing key label {:?} for {:?}",
                    chord.label(),
                    row.name
                );
            }
        }
    }

    #[test]
    fn both_chord_forms_of_a_secondary_global_binding_appear_in_one_row() {
        let md = help_markdown(true);
        for binding in GLOBAL_BINDINGS.iter().filter(|b| {
            b.secondary && matches!(registry::rows::global::adapt(b.cmd), CommandId::Global(_))
        }) {
            let row_id = registry::rows::global::adapt(binding.cmd);
            let primary = GLOBAL_BINDINGS
                .iter()
                .find(|b| !b.secondary && registry::rows::global::adapt(b.cmd) == row_id)
                .expect("primary binding exists");
            let found_row = md
                .lines()
                .find(|line| line.starts_with("| ") && line.contains(&primary.label()));
            assert!(found_row.is_some(), "missing row for {:?}", primary.label());
            let row = found_row.unwrap();
            assert!(
                row.contains(&binding.label()),
                "missing secondary key label {:?} in row {:?}",
                binding.label(),
                row
            );
        }
    }

    #[test]
    fn editor_section_row_count_matches_the_table() {
        let md = help_markdown(true);
        let section = section_of(&md, "Editor");
        let mut unique_cmds: Vec<Command> = Vec::new();
        for binding in EDITOR_BINDINGS {
            if !unique_cmds.contains(&binding.cmd) {
                unique_cmds.push(binding.cmd);
            }
        }
        assert_eq!(row_count(section), unique_cmds.len());
    }

    #[test]
    fn global_section_row_count_matches_one_row_per_command() {
        let md = help_markdown(true);
        let section = section_of(&md, "Global");
        assert_eq!(row_count(section), expected_row_count(is_global));
    }

    #[test]
    fn the_reload_binding_appears_via_the_registry() {
        let md = help_markdown(true);
        let spec = registry::spec(CommandId::Editor(Command::Reload)).expect("reload row exists");
        assert!(
            md.contains(spec.name),
            "missing reload name {:?}",
            spec.name
        );
        assert!(
            md.contains(&RELOAD.label()),
            "missing reload key label {:?}",
            RELOAD.label()
        );
    }

    #[test]
    fn palette_only_commands_show_their_detail_prose_in_the_palette_section() {
        let md = help_markdown(true);
        let section = section_of(&md, "Palette");
        for cmd in [
            PaletteCommand::Language,
            PaletteCommand::TabByName,
            PaletteCommand::Uppercase,
            PaletteCommand::Lowercase,
        ] {
            let spec = registry::spec(CommandId::Palette(cmd)).expect("palette row exists");
            assert!(
                section.contains(spec.detail),
                "missing palette detail {:?} in:\n{section}",
                spec.detail
            );
        }
    }

    #[test]
    fn command_palette_toggle_appears_in_the_global_section() {
        let md = help_markdown(true);
        let section = section_of(&md, "Global");
        assert!(section.contains("command palette"));
    }

    #[test]
    fn a_command_with_two_chords_produces_one_row_listing_both() {
        let primary = GLOBAL_BINDINGS
            .iter()
            .find(|b| b.cmd == GlobalCommand::ToggleLeft && !b.secondary)
            .expect("primary toggle-left binding exists");
        let secondary = GLOBAL_BINDINGS
            .iter()
            .find(|b| b.cmd == GlobalCommand::ToggleLeft && b.secondary)
            .expect("secondary toggle-left binding exists");

        let md = help_markdown(true);
        let section = section_of(&md, "Global");
        let rows: Vec<&str> = section
            .lines()
            .filter(|line| line.starts_with("| ") && line.contains(&primary.label()))
            .collect();

        assert_eq!(rows.len(), 1, "expected exactly one row, got {rows:?}");
        let row = rows.first().unwrap();
        assert!(
            row.contains(&secondary.label()),
            "row {:?} missing secondary label {:?}",
            row,
            secondary.label()
        );
    }

    #[test]
    fn an_unreliable_probe_marks_every_sup_chord_and_none_other() {
        let reliable = help_markdown(true);
        let unreliable = help_markdown(false);
        assert!(!reliable.contains('\u{26a0}'));

        for binding in GLOBAL_BINDINGS {
            let marked = format!("{} \u{26a0}", binding.label());
            if binding.key.mods.sup {
                assert!(
                    unreliable.contains(&marked),
                    "missing \u{26a0} on ⌘ row {:?}",
                    binding.label()
                );
            } else {
                assert!(
                    !unreliable.contains(&marked),
                    "spurious \u{26a0} on non-⌘ row {:?}",
                    binding.label()
                );
            }
        }
    }

    #[test]
    fn same_physical_chord_bound_under_both_shift_encodings_lists_once() {
        let mut delete_line = EDITOR_BINDINGS
            .iter()
            .filter(|b| b.cmd == Command::DeleteLine);
        let shift_form = delete_line
            .next()
            .expect("shift-lowercase delete-line binding");
        let uppercase_form = delete_line
            .next()
            .expect("uppercase-no-shift delete-line binding");
        assert!(
            delete_line.next().is_none(),
            "expected exactly two delete-line bindings"
        );
        assert_eq!(shift_form.label(), uppercase_form.label());

        let md = help_markdown(true);
        let section = section_of(&md, "Editor");
        let row = section
            .lines()
            .find(|line| line.starts_with("| delete line "))
            .expect("delete line row present");
        assert_eq!(*row, format!("| delete line | {} |", shift_form.label()));
    }

    #[test]
    fn the_palette_keys_section_lists_run_accept_and_typing_rows() {
        let md = help_markdown(true);
        let section = section_of(&md, "Palette Keys");

        let enter_label = registry::chords(CommandId::PaletteKey(PaletteKeyCommand::Enter))
            .next()
            .expect("enter chord exists")
            .label();
        let run_row = section
            .lines()
            .find(|line| line.starts_with("| run the command "))
            .expect("run the command row present");
        assert!(run_row.contains(&enter_label));

        let tab_label = registry::chords(CommandId::PaletteKey(PaletteKeyCommand::Tab))
            .next()
            .expect("tab chord exists")
            .label();
        let accept_row = section
            .lines()
            .find(|line| line.starts_with("| accept completion "))
            .expect("accept completion row present");
        assert!(accept_row.contains(&tab_label));

        let typing_row = section
            .lines()
            .find(|line| line.starts_with("| start typing to filter "))
            .expect("start typing to filter row present");
        assert!(typing_row.contains('\u{2014}'));
    }
}
