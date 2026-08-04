//! `help_markdown` — the Help virtual document's content (plan WP7.S1),
//! generated from the real binding tables so the Help doc and each pane's
//! own key handling can never drift apart: `## Global`/`## Explorer`/
//! `## Open Tabs`/`## Editor` are each built by iterating
//! `keymap::GLOBAL_BINDINGS`/`explorer_keys::EXPLORER_BINDINGS`/
//! `opentabs::TABS_BINDINGS`/`keymap::editor_bindings::EDITOR_BINDINGS` —
//! the SAME tables `footer::default_hint_spans` and each pane's
//! `resolve_in` call already read (one source of truth, `Binding::label`
//! the shared display-string chokepoint). `push_rows` iterates every row
//! of a table regardless of `alias`, so `## Global` lists BOTH the ⌘ and
//! `^` form of each focus chord even though the footer's hints collapse
//! the aliased one away.
//!
//! Plan WP6.S7 replaced the former hand-written `## Editor` section (the
//! one hand-maintained key list, kept in sync by hand with `keymap::
//! resolve`'s match arms) with this same reflection pass, once
//! `editor_bindings::EDITOR_BINDINGS` gave the editor's own chords a real
//! table to reflect over — CONSTITUTION §12: "a hand-maintained key list may
//! not exist".

use crate::explorer_keys::EXPLORER_BINDINGS;
use crate::explorer_search::EXPLORER_SEARCH_BINDINGS;
use crate::keymap::editor_bindings::EDITOR_BINDINGS;
use crate::keymap::{Binding, GLOBAL_BINDINGS};
use crate::opentabs::TABS_BINDINGS;

/// Builds the whole Help document's markdown: a `# Help` heading, one
/// `| Key | Action |` table per real binding table.
pub fn help_markdown() -> String {
    let mut out = String::from("# Help\n\n");
    push_table_section(&mut out, "Global", GLOBAL_BINDINGS);
    // The Explorer's keys come from TWO binding sets — ordinary nav/open,
    // and type-to-search — but they are one pane to the reader, so they
    // share a single heading and a single table rather than producing two
    // identically-titled `## Explorer` sections. Both sets are still
    // reflected over (§12: no hand-maintained key list); only the heading
    // is shared.
    push_section_header(&mut out, "Explorer");
    push_rows(&mut out, EXPLORER_BINDINGS);
    push_rows(&mut out, EXPLORER_SEARCH_BINDINGS);
    out.push('\n');
    push_table_section(&mut out, "Open Tabs", TABS_BINDINGS);
    push_table_section(&mut out, "Editor", EDITOR_BINDINGS);
    out
}

/// One `## <title>` section with a two-column markdown table, one row per
/// `bindings` entry, key label first (`Binding::label` — the same helper
/// `footer.rs`'s default hints already reuse; not duplicated here).
fn push_table_section<C: Copy + 'static>(out: &mut String, title: &str, bindings: &[Binding<C>]) {
    push_section_header(out, title);
    push_rows(out, bindings);
    out.push('\n');
}

/// The `## <title>` heading plus the table's header row — split out of
/// `push_table_section` so a pane whose keys live in more than one binding
/// set (the Explorer: nav/open plus type-to-search) can emit ONE heading
/// and feed it from each set in turn, instead of repeating the heading.
fn push_section_header(out: &mut String, title: &str) {
    out.push_str("## ");
    out.push_str(title);
    out.push_str("\n\n| Key | Action |\n| --- | --- |\n");
}

/// One `| key | action |` row per binding, appended to whatever table
/// header is already open. Callable more than once for the same table.
fn push_rows<C: Copy + 'static>(out: &mut String, bindings: &[Binding<C>]) {
    for binding in bindings {
        out.push_str("| ");
        out.push_str(&binding.label());
        out.push_str(" | ");
        out.push_str(binding.help);
        out.push_str(" |\n");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn generates_headings_for_every_section() {
        let md = help_markdown();
        assert!(md.starts_with("# Help\n"));
        for heading in ["## Global", "## Explorer", "## Open Tabs", "## Editor"] {
            assert!(md.contains(heading), "missing {heading:?} in:\n{md}");
        }
    }

    /// The Explorer's two binding sets (nav/open and type-to-search) share
    /// ONE heading and one table — a reader sees a single pane, not two
    /// identically-titled sections — and that table's row count is the sum
    /// of both tables, by reflection rather than a hand-authored number.
    #[test]
    fn the_explorer_section_is_one_table_fed_by_both_binding_sets() {
        let md = help_markdown();
        assert_eq!(
            md.matches("## Explorer").count(),
            1,
            "the Explorer's keys must not split into two headings:\n{md}"
        );
        let section = md
            .split("## Explorer\n\n")
            .nth(1)
            .expect("Explorer section present");
        let row_count = section
            .lines()
            .take_while(|line| line.starts_with("| "))
            .filter(|line| *line != "| Key | Action |" && *line != "| --- | --- |")
            .count();
        assert_eq!(
            row_count,
            EXPLORER_BINDINGS.len() + EXPLORER_SEARCH_BINDINGS.len()
        );
    }

    #[test]
    fn every_global_binding_help_label_and_key_label_appears() {
        let md = help_markdown();
        for binding in GLOBAL_BINDINGS {
            assert!(
                md.contains(binding.help),
                "missing help label {:?}",
                binding.help
            );
            assert!(
                md.contains(&binding.label()),
                "missing key label {:?}",
                binding.label()
            );
        }
    }

    /// Every focus command's alias row (the ⌘/`^` form the footer's hints
    /// collapse away, plan item 5) must still surface in the generated Help
    /// doc — `push_rows` iterates the whole table with no `alias` filter,
    /// so both forms of every pair appear.
    #[test]
    fn both_chord_forms_of_every_aliased_global_binding_appear() {
        let md = help_markdown();
        for binding in GLOBAL_BINDINGS.iter().filter(|b| b.alias) {
            assert!(
                md.contains(&binding.label()),
                "missing aliased key label {:?}",
                binding.label()
            );
        }
    }

    /// Plan WP6 "Done when" gate: no hand-maintained editor key list left
    /// (see this module's doc comment), AND the generated `## Editor`
    /// section's row count equals `EDITOR_BINDINGS.len()` exactly —
    /// reflection, not a hand-authored count that could silently drift
    /// from the table.
    #[test]
    fn editor_section_row_count_matches_the_table() {
        let md = help_markdown();
        let section = md
            .split("## Editor\n\n")
            .nth(1)
            .expect("Editor section present");
        let row_count = section
            .lines()
            .filter(|line| {
                line.starts_with("| ") && *line != "| Key | Action |" && *line != "| --- | --- |"
            })
            .count();
        assert_eq!(row_count, EDITOR_BINDINGS.len());
    }

    /// Plan WP6 "Done when": the generated Help doc contains the reload
    /// binding — read straight off `EDITOR_BINDINGS` itself (via `editing::
    /// RELOAD`, the same constant `Command::Reload`'s dispatch arm binds),
    /// never a hardcoded `"reload image"`/`"⌘R"` literal, so a future
    /// rename of the help text or a rebind to a different chord can't
    /// silently leave this test asserting stale strings.
    #[test]
    fn the_reload_binding_appears_in_the_generated_help_doc() {
        let md = help_markdown();
        let reload = crate::keymap::editor_bindings::RELOAD;
        assert!(
            md.contains(reload.help),
            "missing reload help label {:?}",
            reload.help
        );
        assert!(
            md.contains(&reload.label()),
            "missing reload key label {:?}",
            reload.label()
        );
    }
}
