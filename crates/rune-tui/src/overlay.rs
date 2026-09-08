use crate::app::App;
use crate::filesearch::FileSearchState;
use crate::find::FindState;
use crate::palette::PaletteState;
use crate::pane::Pane;
use crate::runtime::Effects;

#[derive(Default)]
pub(crate) enum Overlay {
    #[default]
    None,
    // Boxed: `FieldState` now carries a full `TextField` (buffer, cursor,
    // undo journal) per field, which would otherwise make `FindState` by
    // far the largest variant and bloat every `Overlay` value to its size.
    Find(Box<FindState>),
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
        crate::find::close(self, false);
        if self.filesearch().is_some() {
            crate::filesearch::cancel(self, effects);
        }
        if self.palette().is_some() {
            crate::palette::close(self);
        }
    }

    pub(crate) fn close_focus_overlays(&mut self, effects: &mut Effects) {
        if self.overlay_owns_focus() {
            self.close_all_overlays(effects);
        }
    }

    // The finder and project search both paint over the left column and
    // force it visible; layout and the splitter treat them identically.
    pub(crate) fn left_column_overlay(&self) -> bool {
        self.filesearch().is_some() || crate::find::project::active(self)
    }

    pub(crate) fn overlay_owns_focus(&self) -> bool {
        match &self.overlay {
            Overlay::Find(state) => state.focused,
            Overlay::FileSearch(_) | Overlay::Palette(_) => true,
            Overlay::None | Overlay::ExplorerFind(_) => false,
        }
    }

    pub(crate) fn find(&self) -> Option<&FindState> {
        match &self.overlay {
            Overlay::Find(state) => Some(state.as_ref()),
            _ => None,
        }
    }

    pub(crate) fn find_mut(&mut self) -> Option<&mut FindState> {
        match &mut self.overlay {
            Overlay::Find(state) => Some(state.as_mut()),
            _ => None,
        }
    }

    pub(crate) fn open_find(&mut self, state: FindState, _: OverlayClearance) {
        self.overlay = Overlay::Find(Box::new(state));
    }

    pub(crate) fn take_find(&mut self) -> Option<FindState> {
        match std::mem::take(&mut self.overlay) {
            Overlay::Find(state) => Some(*state),
            other => {
                self.overlay = other;
                None
            }
        }
    }

    pub fn find_draft(&self) -> Option<&str> {
        self.find().map(|state| state.find.editor.text())
    }

    pub fn replace_draft(&self) -> Option<&str> {
        self.find()
            .and_then(|state| state.replace.as_ref())
            .map(|field| field.editor.text())
    }

    pub fn find_cursor(&self) -> Option<rune_core::cursor::Cursor> {
        self.find().map(|state| state.find.editor.cursor())
    }

    pub fn replace_cursor(&self) -> Option<rune_core::cursor::Cursor> {
        self.find()
            .and_then(|state| state.replace.as_ref())
            .map(|field| field.editor.cursor())
    }

    pub fn replace_field_focused(&self) -> bool {
        self.find()
            .is_some_and(|state| state.focused && state.focus == crate::find::Control::Replace)
    }

    pub fn find_walk_queued(&self) -> Option<usize> {
        let walk = self.find()?.project.as_ref()?.walk.as_ref()?;
        Some(walk.queued.len())
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
