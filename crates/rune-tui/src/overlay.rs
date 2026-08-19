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

/// The read-only accessor a variant's own `&State` getter shares with every
/// other variant: `Some` only while `Overlay` sits on exactly that variant.
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

/// The mutable mirror of [`overlay_get`].
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

/// Installs a freshly built `state` as the one open overlay — gated on an
/// [`OverlayClearance`] so no caller can open one without first proving the
/// Title pane isn't focused.
macro_rules! overlay_open {
    ($fn:ident, $variant:ident, $state:ty) => {
        pub(crate) fn $fn(&mut self, state: $state, _: OverlayClearance) {
            self.overlay = Overlay::$variant(state);
        }
    };
}

/// Drops the overlay outright, only when it's still the named variant —
/// closing an overlay that already changed underneath the caller (or was
/// never open) is a no-op, not a stomp on whatever replaced it.
macro_rules! overlay_close {
    ($fn:ident, $variant:ident) => {
        pub(crate) fn $fn(&mut self) {
            if matches!(self.overlay, Overlay::$variant(_)) {
                self.overlay = Overlay::None;
            }
        }
    };
}

/// Removes and returns the variant's own state, leaving `Overlay::None`
/// behind — for a caller that needs to consume-then-maybe-restore it (a
/// resync, a close that persists something out of the departing state)
/// rather than merely drop it. Any other variant (including `None`) is left
/// untouched.
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
