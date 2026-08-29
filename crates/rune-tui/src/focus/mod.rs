use crate::app::App;
use crate::pane::Pane;
use crate::runtime::Effects;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    Explorer,
    Tabs,
    Editor,
    Title,
    SearchField,
    ReplaceField,
    FileSearch,
    ProjectSearch,
    Palette,
    Messages,
}

pub fn from_pane(pane: Pane) -> FocusTarget {
    match pane {
        Pane::Explorer => FocusTarget::Explorer,
        Pane::Tabs => FocusTarget::Tabs,
        Pane::Editor => FocusTarget::Editor,
        Pane::Title => FocusTarget::Title,
        Pane::Messages => FocusTarget::Messages,
    }
}

pub fn target(app: &App) -> FocusTarget {
    match &app.overlay {
        crate::overlay::Overlay::Search(state) if state.focused => FocusTarget::SearchField,
        crate::overlay::Overlay::FileSearch(_) => FocusTarget::FileSearch,
        crate::overlay::Overlay::ProjectSearch(_) => FocusTarget::ProjectSearch,
        crate::overlay::Overlay::Palette(_) => FocusTarget::Palette,
        _ => from_pane(app.focus()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    Split { explorer: bool, tabs: bool },
    ExplorerOnly,
    EditorOnly,
}

impl LayoutMode {
    fn before_first_resize(left_shown: bool) -> LayoutMode {
        if left_shown {
            LayoutMode::Split {
                explorer: true,
                tabs: true,
            }
        } else {
            LayoutMode::EditorOnly
        }
    }

    pub fn resolve(app: &App) -> LayoutMode {
        if app.frame.is_none() {
            return LayoutMode::before_first_resize(app.splits.left.is_shown());
        }
        crate::layout::resolve_mode(app.frame_area(), app)
    }

    pub fn focusable(self, pane: Pane, messages_open: bool) -> Option<VisiblePane> {
        let painted = match (self, pane) {
            (_, Pane::Title) => true,
            (_, Pane::Messages) => messages_open,
            (LayoutMode::Split { explorer, .. }, Pane::Explorer) => explorer,
            (LayoutMode::Split { tabs, .. }, Pane::Tabs) => tabs,
            (LayoutMode::Split { .. }, Pane::Editor) => true,
            (LayoutMode::EditorOnly, Pane::Editor) => true,
            (LayoutMode::EditorOnly, Pane::Explorer | Pane::Tabs) => false,
            (LayoutMode::ExplorerOnly, Pane::Explorer | Pane::Tabs) => true,
            (LayoutMode::ExplorerOnly, Pane::Editor) => false,
        };
        painted.then_some(VisiblePane(pane))
    }

    fn default_focus(self) -> VisiblePane {
        match self {
            LayoutMode::Split { .. } | LayoutMode::EditorOnly => VisiblePane(Pane::Editor),
            LayoutMode::ExplorerOnly => VisiblePane(Pane::Explorer),
        }
    }

    pub fn focus_or_default(self, pane: Pane, messages_open: bool) -> VisiblePane {
        self.focusable(pane, messages_open)
            .unwrap_or_else(|| self.default_focus())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisiblePane(Pane);

impl VisiblePane {
    pub fn pane(self) -> Pane {
        self.0
    }
}

impl App {
    pub fn focus(&self) -> Pane {
        self.focus
    }

    pub fn layout_mode(&self) -> LayoutMode {
        LayoutMode::resolve(self)
    }

    pub fn focus_title(&mut self) {
        if self.overlay_owns_focus() {
            crate::messages::warn(self, "close the overlay first — Esc");
            return;
        }
        if self.refuse_if_read_only(self.active_doc().read_only) {
            return;
        }
        // A rename mid-merge would leave the title mismatched with the
        // merge-suffixed tab, so renaming stays blocked until merge exits.
        if matches!(self.merge, crate::merge::MergeState::Active { doc, .. } if doc == self.active)
        {
            crate::messages::warn(self, "can't rename while merge is active — ^M to exit");
            return;
        }
        let Some(target) = self
            .layout_mode()
            .focusable(Pane::Title, crate::messages::is_open(self))
        else {
            return;
        };
        let name = crate::title::name_for(self.active_doc());
        self.title.seed(&name);
        self.focus = target.pane();
    }

    pub fn refocus_title(&mut self) {
        if self.overlay_owns_focus() {
            return;
        }
        // An async reply can land after the active document changed under
        // it; refuse rather than park focus on a title that can never commit.
        if self.active_doc().is_read_only() {
            return;
        }
        let Some(target) = self
            .layout_mode()
            .focusable(Pane::Title, crate::messages::is_open(self))
        else {
            return;
        };
        self.focus = target.pane();
    }

    pub fn set_focus(&mut self, next: VisiblePane, effects: &mut Effects) {
        let next = next.pane();
        if self.focus == next {
            return;
        }
        if self.focus == Pane::Title
            && crate::title::on_blur(self, effects) == crate::rename::Commit::Refused
        {
            return;
        }
        if self.focus == Pane::Explorer && next != Pane::Explorer {
            crate::explorer_search::clear_search(self);
        }
        if next == Pane::Explorer {
            self.explorer.browsing_origin = crate::returnto::ReturnTo::to(self.active);
        }
        self.focus = next;
        crate::messages::refresh_focused_flag(self);
    }

    pub fn set_focus_pane(&mut self, pane: Pane, effects: &mut Effects) {
        let target = self
            .layout_mode()
            .focus_or_default(pane, crate::messages::is_open(self));
        self.set_focus(target, effects);
    }

    pub fn blur_title(&mut self, effects: &mut Effects) {
        if self.focus == Pane::Title {
            self.set_focus_pane(Pane::Editor, effects);
        }
    }
}

pub fn reconcile(app: &mut App, effects: &mut Effects) {
    let messages_open = crate::messages::is_open(app);
    if app
        .layout_mode()
        .focusable(app.focus(), messages_open)
        .is_none()
    {
        app.set_focus_pane(app.focus(), effects);
    }
}

#[cfg(test)]
mod tests;
