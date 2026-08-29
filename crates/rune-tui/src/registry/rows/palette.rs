use crate::app::App;

use super::super::{
    ArgKind, Availability, CommandId, CommandSpec, PaletteCommand, language, read_only_edit,
    tab_switch,
};

const fn row(
    cmd: PaletteCommand,
    name: &'static str,
    prose: &'static str,
    arg: ArgKind,
    fuzzy_aliases: &'static [&'static str],
    availability: fn(&App) -> Availability,
) -> CommandSpec {
    CommandSpec {
        id: CommandId::Palette(cmd),
        name,
        fuzzy_aliases,
        help: prose,
        detail: prose,
        arg,
        listed: true,
        availability,
    }
}

pub(crate) static ROWS: &[CommandSpec] = &[
    row(
        PaletteCommand::Language,
        "language",
        "change this document's language for the session",
        ArgKind::Language,
        &["syntax", "lang"],
        language,
    ),
    row(
        PaletteCommand::TabByName,
        "tab",
        "switch to an open tab by name",
        ArgKind::OpenTab,
        &["switch tab"],
        tab_switch,
    ),
    row(
        PaletteCommand::Uppercase,
        "uppercase",
        "uppercase the selection, or the word under the cursor",
        ArgKind::None,
        &["upper case"],
        read_only_edit,
    ),
    row(
        PaletteCommand::Lowercase,
        "lowercase",
        "lowercase the selection, or the word under the cursor",
        ArgKind::None,
        &["lower case"],
        read_only_edit,
    ),
];
