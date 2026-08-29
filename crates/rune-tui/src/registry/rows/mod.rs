use std::sync::LazyLock;

use crate::binding::KeyPattern;

use super::{CommandId, CommandSpec};

pub(crate) mod editor;
pub(crate) mod global;
pub(crate) mod palette;
pub(crate) mod pane;

static REGISTRY: LazyLock<Vec<CommandSpec>> = LazyLock::new(|| {
    let mut all = Vec::new();
    all.extend_from_slice(global::ROWS);
    all.extend_from_slice(editor::ROWS);
    all.extend_from_slice(pane::ROWS);
    all.extend_from_slice(palette::ROWS);
    all
});

pub(crate) fn registry() -> &'static [CommandSpec] {
    &REGISTRY
}

pub(crate) fn chords_for(id: CommandId) -> impl Iterator<Item = KeyPattern> {
    let global = crate::global::GLOBAL_BINDINGS
        .iter()
        .filter(move |b| global::adapt(b.cmd) == id)
        .map(|b| b.key);
    let editor = crate::keymap::editor_bindings::EDITOR_BINDINGS
        .iter()
        .filter(move |b| editor::adapt(b.cmd) == id)
        .map(|b| b.key);
    let explorer = crate::explorer_keys::EXPLORER_BINDINGS
        .iter()
        .filter(move |b| pane::adapt_explorer(b.cmd) == id)
        .map(|b| b.key);
    let explorer_search = crate::explorer_search::EXPLORER_SEARCH_BINDINGS
        .iter()
        .filter(move |b| pane::adapt_explorer_search(b.cmd) == id)
        .map(|b| b.key);
    let tabs = crate::opentabs::TABS_BINDINGS
        .iter()
        .filter(move |b| pane::adapt_tabs(b.cmd) == id)
        .map(|b| b.key);
    let filesearch = crate::filesearch::keys::FILESEARCH_BINDINGS
        .iter()
        .filter(move |b| pane::adapt_filesearch(b.cmd) == id)
        .map(|b| b.key);
    let diff = crate::diff_view::keys::DIFF_BINDINGS
        .iter()
        .filter(move |b| pane::adapt_diff(b.cmd) == id)
        .map(|b| b.key);
    let palette_key = crate::palette::keys::PALETTE_BINDINGS
        .iter()
        .filter(move |b| pane::adapt_palette_key(b.cmd) == id)
        .map(|b| b.key);

    global
        .chain(editor)
        .chain(explorer)
        .chain(explorer_search)
        .chain(tabs)
        .chain(filesearch)
        .chain(diff)
        .chain(palette_key)
}
