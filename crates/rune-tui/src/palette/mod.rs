use ratatui::layout::Rect;

use crate::app::App;
use crate::field::TextField;
use crate::generation::PaletteGen as Generation;
use crate::listnav;
use crate::registry::{Availability, CommandId};
use crate::runtime::{CmdError, Effects};

pub(crate) mod args;
pub(crate) mod keys;
mod rank;
#[cfg(test)]
mod tests;

pub(crate) use args::ArgRow;

pub(crate) const RECENTS_LIMIT: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Recent,
    NameHit,
    HelpHit,
    Unavailable,
}

pub struct PaletteRow {
    pub id: CommandId,
    pub via_alias: Option<&'static str>,
    pub indices: Vec<u32>,
    pub availability: Availability,
    pub tier: Tier,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PaletteMode {
    Name,
    Param { cmd: CommandId, cmd_end: usize },
}

pub struct PaletteState {
    pub field: TextField,
    matcher: nucleo_matcher::Matcher,
    pub mode: PaletteMode,
    pub rows: Vec<PaletteRow>,
    pub arg_rows: Vec<ArgRow>,
    pub nav: listnav::List,
    pub recents: Vec<String>,
    pub generation: Generation,
    pub refusal: Option<String>,
}

impl PaletteState {
    pub(crate) fn active_len(&self) -> usize {
        match self.mode {
            PaletteMode::Name => self.rows.len(),
            PaletteMode::Param { .. } => self.arg_rows.len(),
        }
    }
}

pub(crate) fn open(app: &mut App, effects: &mut Effects) {
    if app.palette().is_some() {
        return;
    }
    let Some(clearance) = app.clear_title_for_overlay(effects) else {
        return;
    };
    crate::search::close(app);
    crate::filesearch::cancel(app, effects);
    let generation = app.next_palette_gen.mint();
    let mut state = PaletteState {
        field: TextField::new(""),
        matcher: nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT),
        mode: PaletteMode::Name,
        rows: Vec::new(),
        arg_rows: Vec::new(),
        nav: listnav::List { cursor: 0, top: 0 },
        recents: Vec::new(),
        generation,
        refusal: None,
    };
    rank::rank(app, &mut state);
    app.open_palette(state, clearance);
    if let Some(db) = app.db.as_ref() {
        effects.cmds.push(crate::runtime::load_command_history_cmd(
            db.store.reader_query(),
            generation,
        ));
    }
}

pub(crate) fn close(app: &mut App) {
    app.close_palette();
}

pub(crate) fn content_rows(state: &PaletteState) -> u16 {
    let show_separator = state.field.is_empty() && !state.recents.is_empty();
    3 + u16::from(state.refusal.is_some()) + u16::from(show_separator)
}

pub(crate) fn capacity(area_height: u16, state: &PaletteState) -> usize {
    let chrome = content_rows(state);
    let max_height = (area_height * 2 / 3).max(chrome.saturating_add(1));
    max_height.saturating_sub(chrome) as usize
}

pub(crate) fn row_capacity(app: &App) -> usize {
    let Some(state) = app.palette() else {
        return 0;
    };
    capacity(app.frame_height, state)
}

pub(crate) fn recompute(app: &mut App) {
    resync(app, true);
}

pub(crate) fn sync_stale(app: &mut App) {
    resync(app, false);
}

fn resync(app: &mut App, clear_refusal: bool) {
    let Some(mut state) = app.take_palette() else {
        return;
    };
    if clear_refusal {
        state.refusal = None;
    }
    match state.mode {
        PaletteMode::Name => {
            state.arg_rows.clear();
            rank::rank(app, &mut state);
        }
        PaletteMode::Param { cmd, cmd_end } => {
            state.rows.clear();
            let query = state
                .field
                .text()
                .get(cmd_end..)
                .unwrap_or("")
                .trim_start()
                .to_string();
            let PaletteState {
                matcher, arg_rows, ..
            } = &mut state;
            *arg_rows = args::rank(app, cmd, &query, matcher);
        }
    }
    let height = capacity(app.frame_height, &state).max(1);
    let margin = (height / 4).min(4);
    let len = state.active_len();
    if len == 0 {
        state.nav.cursor = 0;
    } else {
        state.nav.cursor = state.nav.cursor.min(len - 1);
    }
    state.nav.follow(len, height, margin, 0);
    app.restore_palette(state);
}

pub(crate) fn ghost_text(state: &PaletteState) -> Option<String> {
    let PaletteMode::Param { cmd_end, .. } = state.mode else {
        return None;
    };
    let row = state.arg_rows.get(state.nav.cursor)?;
    let query = state.field.text().get(cmd_end..)?.trim_start();
    args::ghost_suffix(query, &row.label)
}

pub(crate) fn geometry_rect(area: Rect, app: &App) -> Option<Rect> {
    let state = app.palette()?;
    let rows_shown = state.active_len().min(row_capacity(app)) as u16;
    let height = (content_rows(state) + rows_shown).min(area.height);
    let width = area.width.min(76);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height / 6;
    if width == 0 || height == 0 {
        return None;
    }
    Some(Rect::new(x, y, width, height))
}

pub(crate) fn handle_recents_loaded(
    app: &mut App,
    generation: Generation,
    result: Result<Vec<String>, CmdError>,
) {
    let current = app.palette().map(|s| s.generation);
    if current != Some(generation) {
        return;
    }
    match result {
        Ok(names) => {
            if let Some(state) = app.palette_mut() {
                state.recents = names;
            }
        }
        Err(e) => {
            crate::messages::error(app, format!("command history not loaded: {e}"));
        }
    }
    recompute(app);
}
