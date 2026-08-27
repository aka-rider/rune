use crate::binding::KeyPattern;
use crate::registry::{self, CommandId, CommandSpec};

/// `sup_chords_reliable` is `term::sup_chords_reliable(app.keyboard_flags)`
/// at the one production call site (`workspace::show_help`) — this module
/// stays termina-free, so the caller does that lookup and hands over a
/// plain bool. `false` means the probe came back missing a bit this app
/// asked for, so every `⌘` chord below gets an `\u{26a0}` marker: the
/// per-row honesty the disambiguation probe owes the user, without
/// guessing which specific chord they meant to press.
pub fn help_markdown(sup_chords_reliable: bool) -> String {
    let mut out = String::from("# Help\n\n");
    if !sup_chords_reliable {
        out.push_str(
            "_this terminal never confirmed key disambiguation \u{2014} rows marked \u{26a0} may not reach rune here._\n\n",
        );
    }
    push_section(&mut out, "Global", is_global, sup_chords_reliable);
    push_section(&mut out, "Explorer", is_explorer, sup_chords_reliable);
    push_section(&mut out, "File Search", is_file_search, sup_chords_reliable);
    push_section(&mut out, "Open Tabs", is_open_tabs, sup_chords_reliable);
    push_section(&mut out, "Editor", is_editor, sup_chords_reliable);
    push_section(&mut out, "Diff View", is_diff, sup_chords_reliable);
    push_section(&mut out, "Palette", is_palette, sup_chords_reliable);
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

fn push_section(
    out: &mut String,
    title: &str,
    pick: fn(CommandId) -> bool,
    sup_chords_reliable: bool,
) {
    out.push_str("## ");
    out.push_str(title);
    out.push_str("\n\n| Key | Command | Action |\n| --- | --- | --- |\n");
    for row in registry::rows::registry().iter().filter(|row| pick(row.id)) {
        push_command_rows(out, row, sup_chords_reliable);
    }
    out.push('\n');
}

fn push_command_rows(out: &mut String, row: &CommandSpec, sup_chords_reliable: bool) {
    let chords: Vec<KeyPattern> = registry::chords(row.id).collect();
    if chords.is_empty() {
        if row.listed {
            push_row(out, "\u{2014}", row.name, row.help);
        }
        return;
    }
    for chord in chords {
        let mut key = chord.label();
        if chord.mods.sup && !sup_chords_reliable {
            key.push_str(" \u{26a0}");
        }
        push_row(out, &key, row.name, row.help);
    }
}

fn push_row(out: &mut String, key: &str, name: &str, help: &str) {
    out.push_str("| ");
    out.push_str(key);
    out.push_str(" | ");
    out.push_str(name);
    out.push_str(" | ");
    out.push_str(help);
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
                    && *line != "| Key | Command | Action |"
                    && *line != "| --- | --- | --- |"
            })
            .count()
    }

    fn expected_row_count(pick: fn(CommandId) -> bool) -> usize {
        registry::rows::registry()
            .iter()
            .filter(|row| pick(row.id))
            .map(|row| {
                let chords = registry::chords(row.id).count();
                if chords > 0 {
                    chords
                } else if row.listed {
                    1
                } else {
                    0
                }
            })
            .sum()
    }

    #[test]
    fn generates_headings_for_every_section() {
        let md = help_markdown(true);
        assert!(md.starts_with("# Help\n"));
        for heading in [
            "## Global",
            "## Explorer",
            "## File Search",
            "## Open Tabs",
            "## Editor",
            "## Diff View",
            "## Palette",
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
    fn every_included_row_help_and_chord_labels_appear() {
        let md = help_markdown(true);
        for row in registry::rows::registry() {
            let chords: Vec<KeyPattern> = registry::chords(row.id).collect();
            if chords.is_empty() && !row.listed {
                continue;
            }
            assert!(md.contains(row.help), "missing help {:?}", row.help);
            for chord in &chords {
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
    fn both_chord_forms_of_a_secondary_global_binding_appear() {
        let md = help_markdown(true);
        for binding in GLOBAL_BINDINGS.iter().filter(|b| b.secondary) {
            assert!(
                md.contains(&binding.label()),
                "missing secondary key label {:?}",
                binding.label()
            );
        }
    }

    #[test]
    fn editor_section_row_count_matches_the_table() {
        let md = help_markdown(true);
        let section = section_of(&md, "Editor");
        assert_eq!(row_count(section), EDITOR_BINDINGS.len());
    }

    #[test]
    fn global_section_row_count_excludes_tab_switch_rows_moved_to_palette() {
        let md = help_markdown(true);
        let section = section_of(&md, "Global");
        // Trash keeps its listed registry row (reachable from the palette)
        // but has no `GLOBAL_BINDINGS` chord of its own any more — it still
        // renders one row (key column "—"), so the count is one more than
        // the raw binding-table tally.
        let expected = GLOBAL_BINDINGS
            .iter()
            .filter(|b| !matches!(b.cmd, GlobalCommand::TabSwitch(_)))
            .count()
            + 1;
        assert_eq!(row_count(section), expected);
    }

    #[test]
    fn the_reload_binding_appears_via_the_registry() {
        let md = help_markdown(true);
        let spec = registry::spec(CommandId::Editor(Command::Reload)).expect("reload row exists");
        assert!(
            md.contains(spec.help),
            "missing reload help {:?}",
            spec.help
        );
        assert!(
            md.contains(&RELOAD.label()),
            "missing reload key label {:?}",
            RELOAD.label()
        );
    }

    #[test]
    fn palette_only_commands_appear_in_the_palette_section() {
        let md = help_markdown(true);
        let section = section_of(&md, "Palette");
        for name in ["language", "uppercase", "lowercase", "tab"] {
            assert!(
                section.contains(name),
                "missing palette command {name:?} in:\n{section}"
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
}
