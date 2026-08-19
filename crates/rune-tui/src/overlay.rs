use crate::app::App;
use crate::filesearch::FileSearchState;
use crate::palette::PaletteState;
use crate::pane::Pane;
use crate::runtime::Effects;
use crate::search::SearchState;

#[derive(Default)]
pub(crate) enum Overlay {
    #[default]
    None,
    Search(SearchState),
    FileSearch(FileSearchState),
    Palette(PaletteState),
    ExplorerFind(String),
}

pub(crate) struct OverlayClearance(());

impl App {
    pub(crate) fn clear_title_for_overlay(
        &mut self,
        effects: &mut Effects,
    ) -> Option<OverlayClearance> {
        self.blur_title(effects);
        (self.focus() != Pane::Title).then_some(OverlayClearance(()))
    }

    /// Closes every overlay — the in-file search bar whether or not it is
    /// focused, the fuzzy file finder, and the command palette — for every
    /// global that moves focus, switches the active document, or opens
    /// another surface. The finder closes via `filesearch::cancel` rather
    /// than a bare overlay reset, so its own focus/return-to restore stays
    /// coherent instead of leaving `App::focus` wherever the caller's own
    /// arm is about to move it.
    pub(crate) fn close_all_overlays(&mut self, effects: &mut Effects) {
        crate::search::close(self);
        if self.filesearch().is_some() {
            crate::filesearch::cancel(self, effects);
        }
        if self.palette().is_some() {
            crate::palette::close(self);
        }
    }

    /// Closes only an overlay that owns the keyboard, so a Guard taking the
    /// keystroke never also discards a kept, unfocused search highlight.
    /// One `Overlay` slot means at most one is ever open, so closing "all"
    /// once it owns focus closes exactly that one.
    pub(crate) fn close_focus_overlays(&mut self, effects: &mut Effects) {
        if self.overlay_owns_focus() {
            self.close_all_overlays(effects);
        }
    }

    pub(crate) fn overlay_owns_focus(&self) -> bool {
        match &self.overlay {
            Overlay::Search(state) => state.focused,
            Overlay::FileSearch(_) | Overlay::Palette(_) => true,
            Overlay::None | Overlay::ExplorerFind(_) => false,
        }
    }

    pub(crate) fn search(&self) -> Option<&SearchState> {
        match &self.overlay {
            Overlay::Search(state) => Some(state),
            _ => None,
        }
    }

    pub fn search_draft(&self) -> Option<&str> {
        self.search().map(|state| state.draft.as_str())
    }

    pub(crate) fn search_mut(&mut self) -> Option<&mut SearchState> {
        match &mut self.overlay {
            Overlay::Search(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn open_search(&mut self, state: SearchState, _: OverlayClearance) {
        self.overlay = Overlay::Search(state);
    }

    pub(crate) fn take_search(&mut self) -> Option<SearchState> {
        match std::mem::take(&mut self.overlay) {
            Overlay::Search(state) => Some(state),
            other => {
                self.overlay = other;
                None
            }
        }
    }

    pub fn filesearch(&self) -> Option<&FileSearchState> {
        match &self.overlay {
            Overlay::FileSearch(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn filesearch_mut(&mut self) -> Option<&mut FileSearchState> {
        match &mut self.overlay {
            Overlay::FileSearch(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn open_filesearch(&mut self, state: FileSearchState, _: OverlayClearance) {
        self.overlay = Overlay::FileSearch(state);
    }

    pub(crate) fn close_filesearch(&mut self) {
        if matches!(self.overlay, Overlay::FileSearch(_)) {
            self.overlay = Overlay::None;
        }
    }

    pub fn palette(&self) -> Option<&PaletteState> {
        match &self.overlay {
            Overlay::Palette(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn palette_mut(&mut self) -> Option<&mut PaletteState> {
        match &mut self.overlay {
            Overlay::Palette(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn open_palette(&mut self, state: PaletteState, _: OverlayClearance) {
        self.overlay = Overlay::Palette(state);
    }

    pub(crate) fn restore_palette(&mut self, state: PaletteState) {
        self.overlay = Overlay::Palette(state);
    }

    pub(crate) fn close_palette(&mut self) {
        if matches!(self.overlay, Overlay::Palette(_)) {
            self.overlay = Overlay::None;
        }
    }

    pub(crate) fn take_palette(&mut self) -> Option<PaletteState> {
        match std::mem::take(&mut self.overlay) {
            Overlay::Palette(state) => Some(state),
            other => {
                self.overlay = other;
                None
            }
        }
    }

    pub fn explorer_find(&self) -> Option<&str> {
        match &self.overlay {
            Overlay::ExplorerFind(query) => Some(query.as_str()),
            _ => None,
        }
    }

    pub(crate) fn explorer_find_mut(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            Overlay::ExplorerFind(query) => Some(query),
            _ => None,
        }
    }

    pub(crate) fn explorer_find_push(&mut self, c: char) {
        if !matches!(self.overlay, Overlay::ExplorerFind(_)) {
            self.overlay = Overlay::ExplorerFind(String::new());
        }
        if let Overlay::ExplorerFind(query) = &mut self.overlay {
            crate::queryline::type_char(query, c);
        }
    }

    pub(crate) fn close_explorer_find(&mut self) {
        if matches!(self.overlay, Overlay::ExplorerFind(_)) {
            self.overlay = Overlay::None;
        }
    }
}
