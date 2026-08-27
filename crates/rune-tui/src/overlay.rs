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

macro_rules! overlay_get {
    ($vis:vis $fn:ident, $variant:ident, $state:ty) => {
        $vis fn $fn(&self) -> Option<&$state> {
            match &self.overlay {
                Overlay::$variant(state) => Some(state),
                _ => None,
            }
        }
    };
}

macro_rules! overlay_get_mut {
    ($fn:ident, $variant:ident, $state:ty) => {
        pub(crate) fn $fn(&mut self) -> Option<&mut $state> {
            match &mut self.overlay {
                Overlay::$variant(state) => Some(state),
                _ => None,
            }
        }
    };
}

// `OverlayClearance` has no public constructor, so a caller can only reach
// this if it already proved Title isn't focused.
macro_rules! overlay_open {
    ($fn:ident, $variant:ident, $state:ty) => {
        pub(crate) fn $fn(&mut self, state: $state, _: OverlayClearance) {
            self.overlay = Overlay::$variant(state);
        }
    };
}

// Only closes the named variant, so an overlay that already changed
// underneath the caller is left alone rather than stomped.
macro_rules! overlay_close {
    ($fn:ident, $variant:ident) => {
        pub(crate) fn $fn(&mut self) {
            if matches!(self.overlay, Overlay::$variant(_)) {
                self.overlay = Overlay::None;
            }
        }
    };
}

macro_rules! overlay_take {
    ($fn:ident, $variant:ident, $state:ty) => {
        pub(crate) fn $fn(&mut self) -> Option<$state> {
            match std::mem::take(&mut self.overlay) {
                Overlay::$variant(state) => Some(state),
                other => {
                    self.overlay = other;
                    None
                }
            }
        }
    };
}

impl App {
    pub(crate) fn clear_title_for_overlay(
        &mut self,
        effects: &mut Effects,
    ) -> Option<OverlayClearance> {
        self.blur_title(effects);
        (self.focus() != Pane::Title).then_some(OverlayClearance(()))
    }

    // The finder closes through `filesearch::cancel` rather than a bare
    // overlay reset, so its own focus/return-to restore stays coherent.
    pub(crate) fn close_all_overlays(&mut self, effects: &mut Effects) {
        crate::search::close(self);
        if self.filesearch().is_some() {
            crate::filesearch::cancel(self, effects);
        }
        if self.palette().is_some() {
            crate::palette::close(self);
        }
    }

    // Skips a kept, unfocused search highlight; only the overlay that owns
    // the keyboard is closed here.
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

    overlay_get!(pub(crate) search, Search, SearchState);
    overlay_get_mut!(search_mut, Search, SearchState);
    overlay_open!(open_search, Search, SearchState);
    overlay_take!(take_search, Search, SearchState);

    pub fn search_draft(&self) -> Option<&str> {
        self.search().map(|state| state.draft.as_str())
    }

    overlay_get!(pub filesearch, FileSearch, FileSearchState);
    overlay_get_mut!(filesearch_mut, FileSearch, FileSearchState);
    overlay_open!(open_filesearch, FileSearch, FileSearchState);
    overlay_close!(close_filesearch, FileSearch);

    overlay_get!(pub palette, Palette, PaletteState);
    overlay_get_mut!(palette_mut, Palette, PaletteState);
    overlay_open!(open_palette, Palette, PaletteState);
    overlay_close!(close_palette, Palette);
    overlay_take!(take_palette, Palette, PaletteState);

    pub(crate) fn restore_palette(&mut self, state: PaletteState) {
        self.overlay = Overlay::Palette(state);
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

    overlay_close!(close_explorer_find, ExplorerFind);
}
