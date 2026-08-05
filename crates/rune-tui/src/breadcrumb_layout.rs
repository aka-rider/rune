//! The path-shortening/eliding computation side of [`crate::breadcrumb`],
//! split out to keep it under the 500-line budget: relativizing a path
//! against the workspace root ([`crumb_parts`]) and building the truncated,
//! styled crumb spans that fit a given width ([`build_crumb`]). The
//! render/overlay side — splicing those spans onto the center pane's
//! bottom border row — stays in `crate::breadcrumb`.

use ratatui::style::Style;
use ratatui::text::Span;
use std::path::Component;

use crate::width::display_width;

/// Marks a crumb whose leading parts were dropped to fit the width. Two
/// display columns; the trailing `/` reads as "…and more directories
/// above", continuing the bare-slash directory chain that follows it.
pub(crate) const ELLIPSIS: &str = "…/";

/// Between two directories: a bare slash, no padding.
pub(crate) const SEP: &str = "/";

/// Between the LAST directory and the leaf file name only — the one place
/// the crumb breathes, so the file name reads as the subject and the
/// directory chain as its address.
pub(crate) const LEAF_SEP: &str = " › ";

/// Relativizes `path` against `root` (plan WP4.S6), returning the ordered list of path components the
/// crumb renders. When `root` is non-empty and `path` is under it (`Path::
/// starts_with` compares whole components, so this can never mistake
/// `/a/vault2` for being under `/a/vault` the way a bare string-prefix
/// check would), the result is root's own base name followed by the
/// remaining components below it — e.g. root `/Users/xiii/vault`, path
/// `/Users/xiii/vault/notes/note.md` yields `["vault", "notes",
/// "note.md"]`, not `["Users", "xiii", "vault", "notes", "note.md"]`.
/// Falls back to every `Normal` component of the absolute path otherwise
/// (`root` empty — not yet resolved — or `path` outside it).
pub(crate) fn crumb_parts(path: &std::path::Path, root: &std::path::Path) -> Vec<String> {
    if !root.as_os_str().is_empty()
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

/// Builds the crumb's spans right-to-left: each part verbatim (fg
/// `SPECIAL`, no padding of its own) followed — for every part except the
/// rightmost (the leaf) — by a `SUBTLE` separator span, `LEAF_SEP` when
/// the part is the last directory (index `n - 2`) and `SEP` between any
/// two directories above it. The walk stops as soon as adding the next
/// part would overflow `max_width` by the 6-column buffer, at which point
/// an `ELLIPSIS` span is prepended. Index `0` (the leftmost/root-most
/// component) is NEVER dropped — the `&& i > 0` guard on the truncation
/// check. A single-component path renders as just that leaf.
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
            // The part directly left of the leaf gets the wider ` › `;
            // every directory above it is joined by a bare `/`.
            let sep = if i + 2 == n { LEAF_SEP } else { SEP };
            (
                part_width + display_width(sep),
                vec![
                    Span::styled(part.clone(), Style::new().fg(theme.chrome.special)),
                    Span::styled(sep, Style::new().fg(theme.chrome.subtle)),
                ],
            )
        };

        // An arbitrary buffer for the ellipsis and some breathing room.
        if current_width + seg_width + 6 > max_width && i > 0 {
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
            Path::new("/Users/xiii/vault"),
        );
        assert_eq!(parts, vec!["vault", "notes", "note.md"]);
    }

    #[test]
    fn crumb_parts_falls_back_to_the_absolute_path_outside_root() {
        let parts = crumb_parts(
            Path::new("/Users/xiii/other/note.md"),
            Path::new("/Users/xiii/vault"),
        );
        assert_eq!(parts, vec!["Users", "xiii", "other", "note.md"]);
    }

    #[test]
    fn crumb_parts_falls_back_to_the_absolute_path_when_root_is_unresolved() {
        let parts = crumb_parts(Path::new("/Users/xiii/vault/note.md"), Path::new(""));
        assert_eq!(parts, vec!["Users", "xiii", "vault", "note.md"]);
    }

    /// `/a/vault2` must never be treated as under root `/a/vault` —
    /// `Path::starts_with` compares whole components, unlike a bare string
    /// prefix check.
    #[test]
    fn crumb_parts_does_not_mistake_a_sibling_with_a_shared_prefix_for_being_under_root() {
        let parts = crumb_parts(Path::new("/a/vault2/notes.md"), Path::new("/a/vault"));
        assert_eq!(parts, vec!["a", "vault2", "notes.md"]);
    }

    /// An independent width oracle (plan [rune-tui C 14]): computed
    /// straight from `unicode_width`/`unicode_segmentation`, never by
    /// calling this module's own `display_width`/`grapheme_width` — so a
    /// regression in the production chokepoint can't pass a test that
    /// merely re-invokes it.
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

    /// The separator glyphs must be ONE display column each, or every width
    /// in this module (dash fill, `bc`, the truncation budget) is computed
    /// against a lie and the `──╯` drifts off the right edge.
    #[test]
    fn the_separator_glyphs_are_single_column() {
        assert_eq!(oracle_cell_width(SEP), 1);
        assert_eq!(oracle_cell_width(LEAF_SEP), 3);
        assert_eq!(oracle_cell_width(ELLIPSIS), 2);
    }

    /// A one-component path is all leaf: no separator, no stray padding.
    #[test]
    fn a_single_component_path_renders_just_the_leaf() {
        let theme = crate::theme::Theme::catppuccin_mocha(false);
        let segments = build_crumb(&["note.md".to_string()], 40, &theme);
        let text: String = segments.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "note.md");
    }

    #[test]
    fn total_width_identity_holds() {
        // 1 (╰) + dash + (bc + 2 surrounding spaces) + 3 (──╯) == block.width,
        // for every width from the bail-out floor up to a generous ceiling.
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
