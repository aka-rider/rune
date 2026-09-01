use std::borrow::Cow;

use rune_core::assert_invariant;

use crate::app::App;
use crate::binding::KeyPattern;
use crate::diff_view::keys::DiffCommand;
use crate::explorer_keys::ExplorerCommand;
use crate::explorer_search::ExplorerSearchCommand;
use crate::filesearch::keys::FileSearchCommand;
use crate::global::GlobalCommand;
use crate::keymap;
use crate::opentabs::TabsCommand;
use crate::palette::keys::PaletteKeyCommand;
use crate::projectsearch::keys::ProjectSearchCommand;

mod avail;
#[cfg(test)]
mod avail_tests;
mod exec;
pub(crate) mod rows;
#[cfg(test)]
mod tests;

pub(crate) use avail::{
    always, language, merge, read_only_edit, reload, save, tab_switch, toggle_read_only,
};
pub(crate) use exec::{ExecOutcome, ResolvedArg, execute};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CommandId {
    Global(GlobalCommand),
    Editor(keymap::Command),
    Explorer(ExplorerCommand),
    ExplorerSearch(ExplorerSearchCommand),
    Tabs(TabsCommand),
    FileSearch(FileSearchCommand),
    ProjectSearch(ProjectSearchCommand),
    Diff(DiffCommand),
    Palette(PaletteCommand),
    PaletteKey(PaletteKeyCommand),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteCommand {
    Language,
    TabByName,
    Uppercase,
    Lowercase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArgKind {
    None,
    Language,
    OpenTab,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Availability {
    Available,
    Unavailable(Cow<'static, str>),
}

#[derive(Clone, Copy)]
pub struct CommandSpec {
    pub id: CommandId,
    pub name: &'static str,
    pub(crate) fuzzy_aliases: &'static [&'static str],
    pub help: &'static str,
    pub detail: &'static str,
    pub(crate) arg: ArgKind,
    pub listed: bool,
    pub(crate) availability: fn(&App) -> Availability,
}

pub(crate) fn spec(id: CommandId) -> Option<&'static CommandSpec> {
    let found = rows::registry().iter().find(|row| row.id == id);
    assert_invariant!(found.is_some(), || format!("no registry row for {id:?}"));
    found
}

pub(crate) fn chords(id: CommandId) -> impl Iterator<Item = KeyPattern> {
    rows::chords_for(id)
}

pub fn all() -> &'static [CommandSpec] {
    rows::registry()
}

pub(crate) fn availability(app: &App, id: CommandId) -> Availability {
    match spec(id) {
        Some(spec) => (spec.availability)(app),
        None => Availability::Unavailable("no such command".into()),
    }
}
