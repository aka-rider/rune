use std::ops::Range;

use rune_core::coords::DisplayRow;
use rune_core::cursor::CursorSet;

use crate::app::App;
use crate::document::DocumentId;
use crate::field::TextField;
use crate::find::matcher::{MatchOptions, Matcher, PatternError};
use crate::find::{globs, project};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FilterKind {
    ThisFile,
    Workspace,
}

pub(crate) enum Filter {
    ThisFile,
    Workspace(project::ProjectResults),
}

impl Filter {
    pub(crate) fn kind(&self) -> FilterKind {
        match self {
            Filter::ThisFile => FilterKind::ThisFile,
            Filter::Workspace(_) => FilterKind::Workspace,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    Find,
    Replace,
    Include,
    Exclude,
    Results,
}

impl Control {
    pub(crate) fn is_field(self) -> bool {
        self != Control::Results
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChipKind {
    ThisFile,
    Case,
    Word,
    Regex,
    HistoryScope,
    ReplaceOne,
    ReplaceAll,
}

#[derive(Debug, Default)]
pub(crate) struct FieldState {
    pub editor: TextField,
    pub history: Vec<String>,
    pub history_pos: Option<usize>,
    pub history_draft: Option<String>,
}

impl FieldState {
    // A seeded selection is inserted whole and then selected end-to-start,
    // so the very next keystroke — a plain typed character, replacing the
    // selection — overwrites it rather than appending after it.
    pub(crate) fn seeded(text: String) -> FieldState {
        let mut editor = TextField::new(&text);
        editor.set_cursor(editor.len(), 0);
        FieldState {
            editor,
            ..FieldState::default()
        }
    }

    pub(crate) fn leave_history(&mut self) {
        self.history_pos = None;
        self.history_draft = None;
    }
}

#[derive(Clone)]
pub(crate) struct Origin {
    pub doc: DocumentId,
    pub cursors: CursorSet,
    pub scroll_row: DisplayRow,
    pub buffer_version: u64,
}

impl Origin {
    pub(crate) fn capture(app: &App) -> Origin {
        let doc = app.active_doc();
        Origin {
            doc: app.active,
            cursors: doc.cursors().clone(),
            scroll_row: doc.viewport.scroll_row,
            buffer_version: doc.buffer.version(),
        }
    }
}

pub(crate) struct FindState {
    pub focused: bool,
    pub options: MatchOptions,
    pub focus: Control,
    pub find: FieldState,
    pub replace: Option<FieldState>,
    pub pattern: Result<Matcher, PatternError>,
    pub origin: Origin,
    pub doc: DocumentId,
    pub buffer_version: u64,
    pub matches: Vec<Range<usize>>,
    pub current: Option<usize>,
    pub filter: Filter,
    pub glob_fields: globs::GlobFields,
    pub history_generation: crate::generation::SearchHistoryGen,
    pub replace_history_generation: crate::generation::ReplaceHistoryGen,
}

impl FindState {
    pub(crate) fn filter_kind(&self) -> FilterKind {
        self.filter.kind()
    }

    pub(crate) fn project(&self) -> Option<&project::ProjectResults> {
        match &self.filter {
            Filter::Workspace(results) => Some(results),
            Filter::ThisFile => None,
        }
    }

    pub(crate) fn project_mut(&mut self) -> Option<&mut project::ProjectResults> {
        match &mut self.filter {
            Filter::Workspace(results) => Some(results),
            Filter::ThisFile => None,
        }
    }

    pub(crate) fn globs(&self) -> Option<&globs::GlobFields> {
        (self.filter_kind() == FilterKind::Workspace).then_some(&self.glob_fields)
    }

    pub(crate) fn results_listed(&self) -> bool {
        self.project()
            .is_some_and(|project| project.results.len() > 1)
    }

    pub(crate) fn settle_focus(&mut self) {
        if !self.control_ring().contains(&self.focus) {
            self.focus = Control::Find;
        }
    }

    pub(crate) fn field_mut(&mut self, control: Control) -> Option<&mut FieldState> {
        let workspace = self.filter_kind() == FilterKind::Workspace;
        match control {
            Control::Find => Some(&mut self.find),
            Control::Replace => self.replace.as_mut(),
            Control::Include if workspace => Some(&mut self.glob_fields.include),
            Control::Exclude if workspace => Some(&mut self.glob_fields.exclude),
            Control::Include | Control::Exclude | Control::Results => None,
        }
    }

    pub(crate) fn focused_field_mut(&mut self) -> Option<&mut FieldState> {
        self.field_mut(self.focus)
    }

    pub(crate) fn control_ring(&self) -> Vec<Control> {
        let mut ring = vec![Control::Find];
        if self.replace.is_some() {
            ring.push(Control::Replace);
        }
        if self.globs().is_some() {
            ring.push(Control::Include);
            ring.push(Control::Exclude);
        }
        if self.results_listed() {
            ring.push(Control::Results);
        }
        ring
    }

    pub(crate) fn chip_on(&self, kind: ChipKind) -> Option<bool> {
        match kind {
            ChipKind::Case => Some(self.options.case_sensitive),
            ChipKind::Word => Some(self.options.whole_word),
            ChipKind::Regex => Some(self.options.regex),
            ChipKind::ThisFile => Some(self.filter_kind() == FilterKind::ThisFile),
            ChipKind::HistoryScope | ChipKind::ReplaceOne | ChipKind::ReplaceAll => None,
        }
    }
}
