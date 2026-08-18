use crate::app::App;
use crate::filesearch::FileSearchState;
use crate::palette::PaletteState;
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

impl App {
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

    pub(crate) fn open_search(&mut self, state: SearchState) {
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

    pub(crate) fn open_filesearch(&mut self, state: FileSearchState) {
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

    pub(crate) fn open_palette(&mut self, state: PaletteState) {
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
            query.push(c);
        }
    }

    pub(crate) fn close_explorer_find(&mut self) {
        if matches!(self.overlay, Overlay::ExplorerFind(_)) {
            self.overlay = Overlay::None;
        }
    }
}
