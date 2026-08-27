use crate::keymap::GlobalCommand;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BarPolicy {
    CloseBars,
    ToggleSearch,
    LeaveOpen,
}

pub(crate) fn bar_policy(cmd: GlobalCommand) -> BarPolicy {
    match cmd {
        GlobalCommand::ToggleLeft
        | GlobalCommand::FocusTitle
        | GlobalCommand::FocusTabs
        | GlobalCommand::ToggleMessages
        | GlobalCommand::Merge
        | GlobalCommand::Help
        | GlobalCommand::NewDocument
        | GlobalCommand::TabSwitch(_)
        | GlobalCommand::CloseFile
        | GlobalCommand::NavBack
        | GlobalCommand::NavForward
        | GlobalCommand::Save => BarPolicy::CloseBars,
        GlobalCommand::ToggleSearch => BarPolicy::ToggleSearch,
        GlobalCommand::QuitChord(_)
        | GlobalCommand::ToggleReadOnly
        | GlobalCommand::Trash
        | GlobalCommand::SearchNext
        | GlobalCommand::SearchPrev
        | GlobalCommand::TogglePin
        | GlobalCommand::ToggleFileSearch
        | GlobalCommand::TogglePalette => BarPolicy::LeaveOpen,
    }
}
