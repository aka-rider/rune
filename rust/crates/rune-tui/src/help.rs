//! `help_markdown` — the Help virtual document's content (plan WP7.S1),
//! generated from the real binding tables so the Help doc and each pane's
//! own key handling can never drift apart: `## Global`/`## Explorer`/
//! `## Open Tabs` are each built by iterating `keymap::GLOBAL_BINDINGS`/
//! `explorer::EXPLORER_BINDINGS`/`opentabs::TABS_BINDINGS` — the SAME
//! tables `footer::default_hint_spans` and each pane's `resolve_in` call
//! already read (one source of truth, `KeyPattern::label` the shared
//! display-string chokepoint).
//!
//! `## Editor` is the one recorded exception (plan decision 9: "`resolve()`
//! ... converts to tables only where WP7 (Help doc) needs enumeration" —
//! it never did, so this section is hand-written instead. Keep it in sync
//! by hand with `keymap::resolve`'s match arms until the editor's own
//! chord set is tabled like the other three panes'.

use crate::explorer::EXPLORER_BINDINGS;
use crate::keymap::{Binding, GLOBAL_BINDINGS};
use crate::opentabs::TABS_BINDINGS;

/// Builds the whole Help document's markdown: a `# Help` heading, one
/// `| Key | Action |` table per real binding table, plus the hand-written
/// `## Editor` section.
pub fn help_markdown() -> String {
    let mut out = String::from("# Help\n\n");
    push_table_section(&mut out, "Global", GLOBAL_BINDINGS);
    push_table_section(&mut out, "Explorer", EXPLORER_BINDINGS);
    push_table_section(&mut out, "Open Tabs", TABS_BINDINGS);
    push_editor_section(&mut out);
    out
}

/// One `## <title>` section with a two-column markdown table, one row per
/// `bindings` entry, key label first (`KeyPattern::label` — the same
/// helper `footer.rs`'s default hints already reuse; not duplicated here).
fn push_table_section<C: Copy + 'static>(out: &mut String, title: &str, bindings: &[Binding<C>]) {
    out.push_str("## ");
    out.push_str(title);
    out.push_str("\n\n| Key | Action |\n| --- | --- |\n");
    for binding in bindings {
        out.push_str("| ");
        out.push_str(&binding.key.label());
        out.push_str(" | ");
        out.push_str(binding.help);
        out.push_str(" |\n");
    }
    out.push('\n');
}

/// The hand-written `## Editor` section (plan decision 9's recorded
/// exception, WP7.S1) — the chords `keymap::resolve`'s match-based
/// resolver owns: navigation, selection (the same chords with Shift held),
/// delete, indent, clipboard, and undo/redo. `Save`/the quit chords are
/// deliberately omitted here — they already have their own `## Global`
/// rows (`GLOBAL_BINDINGS`'s stage-2 pipeline resolves them before the
/// editor's own resolver ever sees them; see `app::handle_editor_key`'s
/// `Command::QuitConfirm` arm doc comment).
fn push_editor_section(out: &mut String) {
    out.push_str("## Editor\n\n");
    const ROWS: &[(&str, &str)] = &[
        ("\u{2190} / \u{2192}", "move left / right"),
        ("\u{2191} / \u{2193}", "move up / down"),
        ("\u{2325}\u{2190} / \u{2325}\u{2192}", "word left / right"),
        ("Home / End", "line start / end"),
        ("PageUp / PageDown, ^U / ^D", "page up / down"),
        (
            "\u{21e7}\u{2190} / \u{21e7}\u{2192}",
            "select char left / right",
        ),
        (
            "\u{21e7}\u{2191} / \u{21e7}\u{2193}",
            "select line up / down",
        ),
        (
            "\u{21e7}\u{2325}\u{2190} / \u{21e7}\u{2325}\u{2192}",
            "select word left / right",
        ),
        ("\u{21e7}Home / \u{21e7}End", "select to line start / end"),
        ("\u{21e7}PageUp / \u{21e7}PageDown", "select page up / down"),
        ("\u{2318}A / ^A", "select all"),
        ("Backspace", "delete left"),
        ("Delete", "delete right"),
        ("Tab", "indent"),
        ("\u{21e7}Tab", "outdent"),
        ("\u{2318}C / ^\u{21e7}C", "copy"),
        ("\u{2318}X", "cut"),
        ("\u{2318}V", "paste"),
        ("\u{2318}Z / ^Z", "undo"),
        ("\u{2318}\u{21e7}Z / ^Y", "redo"),
    ];
    out.push_str("| Key | Action |\n| --- | --- |\n");
    for (key, action) in ROWS {
        out.push_str("| ");
        out.push_str(key);
        out.push_str(" | ");
        out.push_str(action);
        out.push_str(" |\n");
    }
    out.push('\n');
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
                md.contains(&binding.key.label()),
                "missing key label {:?}",
                binding.key.label()
            );
        }
    }
}
