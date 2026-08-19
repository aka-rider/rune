use crate::action::Action;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Keyword {
    Key,
    Mouse,
    Type,
    Paste,
    OpenFileSearch,
    Resize,
    ClipboardReply,
    ConfirmTimeout,
    StaleConfirmTimeout,
    Deliver,
    FailNextSave,
    DirLoaded,
    Highlight,
    DivergeDisk,
    DeliverDb,
    DeliverDbAll,
    HighlightTree,
    AdvanceClock,
    PaletteRecentsLoaded,
    InstallDiffLeft,
}

impl Keyword {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Keyword::Key => "key",
            Keyword::Mouse => "mouse",
            Keyword::Type => "type",
            Keyword::Paste => "paste",
            Keyword::OpenFileSearch => "open-filesearch",
            Keyword::Resize => "resize",
            Keyword::ClipboardReply => "clip",
            Keyword::ConfirmTimeout => "confirm-timeout",
            Keyword::StaleConfirmTimeout => "stale-confirm-timeout",
            Keyword::Deliver => "deliver",
            Keyword::FailNextSave => "fail-next-save",
            Keyword::DirLoaded => "dirloaded",
            Keyword::Highlight => "highlight",
            Keyword::DivergeDisk => "diverge-disk",
            Keyword::DeliverDb => "deliver-db",
            Keyword::DeliverDbAll => "deliver-db-all",
            Keyword::HighlightTree => "highlight-tree",
            Keyword::AdvanceClock => "advance-clock",
            Keyword::PaletteRecentsLoaded => "palette-recents",
            Keyword::InstallDiffLeft => "install-diff-left",
        }
    }

    pub(super) fn for_action(action: &Action) -> Self {
        match action {
            Action::Key(_) => Keyword::Key,
            Action::Mouse(_) => Keyword::Mouse,
            Action::Type(_) => Keyword::Type,
            Action::Paste(_) => Keyword::Paste,
            Action::OpenFileSearch => Keyword::OpenFileSearch,
            Action::Resize(_, _) => Keyword::Resize,
            Action::ClipboardReply(_) => Keyword::ClipboardReply,
            Action::ConfirmTimeout => Keyword::ConfirmTimeout,
            Action::StaleConfirmTimeout(_) => Keyword::StaleConfirmTimeout,
            Action::Deliver => Keyword::Deliver,
            Action::FailNextSave => Keyword::FailNextSave,
            Action::DirLoaded { .. } => Keyword::DirLoaded,
            Action::Highlight { .. } => Keyword::Highlight,
            Action::DivergeDisk => Keyword::DivergeDisk,
            Action::DeliverDb => Keyword::DeliverDb,
            Action::DeliverDbAll => Keyword::DeliverDbAll,
            Action::HighlightTree { .. } => Keyword::HighlightTree,
            Action::AdvanceClock(_) => Keyword::AdvanceClock,
            Action::PaletteRecentsLoaded { .. } => Keyword::PaletteRecentsLoaded,
            Action::InstallDiffLeft { .. } => Keyword::InstallDiffLeft,
        }
    }
}

pub(super) const DIRLOADED_ENTRY: &str = "dirloaded-entry";
pub(super) const HIGHLIGHT_SPAN: &str = "highlight-span";
pub(super) const PALETTE_RECENTS_NAME: &str = "palette-recents-name";
