use ratatui::style::Style;
use ratatui::text::Span;
use std::path::Component;

use crate::footer_hints::hint_entry_spans;
use crate::global::{GlobalCommand, hint_for};
use crate::navhistory::NavHistory;
use crate::theme::Theme;
use crate::width::display_width;

pub(crate) const ELLIPSIS: &str = "…/";
pub(crate) const SEP: &str = "/";
pub(crate) const LEAF_SEP: &str = " › ";

const TRUNCATION_BUFFER: usize = 6;

pub(crate) fn crumb_parts(path: &std::path::Path, root: Option<&std::path::Path>) -> Vec<String> {
    // `strip_prefix` compares whole path components, so `/a/vault2` is
    // never mistaken for being under root `/a/vault` the way a bare
    // string-prefix check would.
    if let Some(root) = root
        && let Ok(remainder) = path.strip_prefix(root)
    {
        let mut parts = Vec::new();
        if let Some(name) = root.file_name() {
            parts.push(name.to_string_lossy().into_owned());
        }
        parts.extend(remainder.components().filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        }));
        return parts;
    }
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// Builds the crumb's spans right-to-left, prepending an `ELLIPSIS` span
/// once the next part would overflow `max_width`. Neither the leaf nor
/// index `0` is ever dropped this way: a crumb naming no file is worth no
/// columns at all, and `overlay` drops the whole crumb instead when even
/// that shortest form does not fit.
pub(crate) fn build_crumb(
    parts: &[String],
    max_width: usize,
    theme: &crate::theme::Theme,
) -> Vec<Span<'static>> {
    let n = parts.len();
    let mut segments: Vec<Span<'static>> = Vec::with_capacity(n * 2);
    let mut current_width = 0usize;

    for (i, part) in parts.iter().enumerate().rev() {
        let part_width = display_width(part);
        let is_last = i == n - 1;

        let (seg_width, seg): (usize, Vec<Span<'static>>) = if is_last {
            (
                part_width,
                vec![Span::styled(
                    part.clone(),
                    Style::new().fg(theme.chrome.special),
                )],
            )
        } else {
            let sep = if i + 2 == n { LEAF_SEP } else { SEP };
            (
                part_width + display_width(sep),
                vec![
                    Span::styled(part.clone(), Style::new().fg(theme.chrome.special)),
                    Span::styled(sep, Style::new().fg(theme.chrome.subtle)),
                ],
            )
        };

        if current_width + seg_width + TRUNCATION_BUFFER > max_width && i > 0 && !is_last {
            segments.insert(
                0,
                Span::styled(ELLIPSIS, Style::new().fg(theme.chrome.special)),
            );
            return segments;
        }

        for span in seg.into_iter().rev() {
            segments.insert(0, span);
        }
        current_width += seg_width;
    }

    segments
}

pub(crate) fn build_controls(history: &NavHistory, theme: &Theme) -> Vec<Span<'static>> {
    [
        (GlobalCommand::NavBack, history.can_back()),
        (GlobalCommand::NavForward, history.can_forward()),
    ]
    .into_iter()
    .filter_map(|(cmd, available)| hint_for(cmd).map(|(label, help)| (label, help, available)))
    .enumerate()
    .flat_map(|(index, (label, help, available))| {
        hint_entry_spans(theme, index, label, help, available)
    })
    .map(without_footer_background)
    .collect()
}

fn without_footer_background(mut span: Span<'static>) -> Span<'static> {
    span.style.bg = None;
    span
}

pub(crate) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::path::Path;
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn crumb_parts_relativizes_against_a_set_root() {
        let parts = crumb_parts(
            Path::new("/Users/xiii/vault/notes/note.md"),
            Some(Path::new("/Users/xiii/vault")),
        );
        assert_eq!(parts, vec!["vault", "notes", "note.md"]);
    }

    #[test]
    fn crumb_parts_falls_back_to_the_absolute_path_outside_root() {
        let parts = crumb_parts(
            Path::new("/Users/xiii/other/note.md"),
            Some(Path::new("/Users/xiii/vault")),
        );
        assert_eq!(parts, vec!["Users", "xiii", "other", "note.md"]);
    }

    #[test]
    fn crumb_parts_falls_back_to_the_absolute_path_when_root_is_unresolved() {
        let parts = crumb_parts(Path::new("/Users/xiii/vault/note.md"), None);
        assert_eq!(parts, vec!["Users", "xiii", "vault", "note.md"]);
    }

    #[test]
    fn crumb_parts_does_not_mistake_a_sibling_with_a_shared_prefix_for_being_under_root() {
        let parts = crumb_parts(Path::new("/a/vault2/notes.md"), Some(Path::new("/a/vault")));
        assert_eq!(parts, vec!["a", "vault2", "notes.md"]);
    }

    // An independent width oracle: computed straight from `unicode_width`/
    // `unicode_segmentation`, never via this module's own `display_width`,
    // so a regression in the production chokepoint can't pass a test that
    // merely re-invokes it.
    fn oracle_cell_width(s: &str) -> usize {
        s.graphemes(true)
            .map(|g| {
                g.chars()
                    .filter_map(unicode_width::UnicodeWidthChar::width)
                    .max()
                    .unwrap_or(0)
                    .max(1)
            })
            .sum()
    }

    #[test]
    fn the_separator_glyphs_are_single_column() {
        assert_eq!(oracle_cell_width(SEP), 1);
        assert_eq!(oracle_cell_width(LEAF_SEP), 3);
        assert_eq!(oracle_cell_width(ELLIPSIS), 2);
    }

    #[test]
    fn a_single_component_path_renders_just_the_leaf() {
        let theme = crate::theme::Theme::catppuccin_mocha(false);
        let segments = build_crumb(&["note.md".to_string()], 40, &theme);
        let text: String = segments.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "note.md");
    }

    #[test]
    fn total_width_identity_holds() {
        let theme = crate::theme::Theme::catppuccin_mocha(false);
        for width in 30u16..80 {
            let parts: Vec<String> = vec!["a".into(), "b".into(), "note.md".into()];
            let segments = build_crumb(&parts, width as usize, &theme);
            let bc: usize = segments
                .iter()
                .map(|s| display_width(s.content.as_ref()))
                .sum();
            if bc + 7 > width as usize {
                continue;
            }
            let dash = width as usize - bc - 6;
            assert_eq!(1 + dash + (bc + 2) + 3, width as usize);
        }
    }
}
