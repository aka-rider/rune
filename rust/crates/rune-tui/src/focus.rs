//! `FocusTarget` — the `when`-clause-facing view of "what has focus" (plan
//! WP6.S3), deliberately a SEPARATE type from `pane::Pane`. `Pane` stays the
//! chrome-region discriminant `app::handle_key`'s stage-3 dispatch already
//! keys off of; `FocusTarget` is the vocabulary `when.rs` clauses are
//! written against, and it needs variants `Pane` doesn't have (the
//! search/replace fields land in WP8).

use crate::pane::Pane;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    Explorer,
    Tabs,
    Editor,
    /// The editable title field (`title.rs`, `Pane::Title`) — reachable
    /// today via `^r`/`␣R` or the Up-at-editor-top gesture.
    Title,
    /// Not yet reachable — WP8 adds the search UI this pairs with. Included
    /// now so a `when` clause authored against it (e.g. a future search-bar
    /// binding table) parses and validates today, ahead of the field that
    /// will actually produce it.
    SearchField,
    /// Not yet reachable — see `SearchField`'s doc; WP8's replace field.
    ReplaceField,
}

/// Derives today's `FocusTarget` from the chrome-level `Pane` — the only
/// input that exists before WP8's search state lands. Once a search bar
/// exists to focus, its own state (not `Pane`, which never grows a
/// `Pane::Search` variant per the plan's decision 7) becomes a second input
/// this function checks first.
pub fn from_pane(pane: Pane) -> FocusTarget {
    match pane {
        Pane::Explorer => FocusTarget::Explorer,
        Pane::Tabs => FocusTarget::Tabs,
        Pane::Editor => FocusTarget::Editor,
        Pane::Title => FocusTarget::Title,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn derives_from_each_pane() {
        assert_eq!(from_pane(Pane::Explorer), FocusTarget::Explorer);
        assert_eq!(from_pane(Pane::Tabs), FocusTarget::Tabs);
        assert_eq!(from_pane(Pane::Editor), FocusTarget::Editor);
        assert_eq!(from_pane(Pane::Title), FocusTarget::Title);
    }
}
