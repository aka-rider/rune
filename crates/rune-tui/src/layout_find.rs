use std::borrow::Cow;

use ratatui::layout::Rect;

use crate::find::bindings::{chip_command, label_for};
use crate::find::{ChipKind, FindState};
use crate::region::Region;
use crate::width::display_width;

pub(crate) const HISTORY_SCOPE_LABEL: &str = "Doc|All";

pub const FIND_PANEL_MIN_BOX_W: u16 = 12;
const FIND_ONLY_ROWS: u16 = 3;
const WITH_REPLACE_ROWS: u16 = 5;
const GLOBS_ROW: u16 = 1;
const GLOB_GAP: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Chip {
    pub kind: ChipKind,
    pub label: &'static str,
}

pub(crate) const FIND_ROW_CHIPS: [Chip; 5] = [
    Chip {
        kind: ChipKind::ThisFile,
        label: "This file",
    },
    Chip {
        kind: ChipKind::Case,
        label: "Aa",
    },
    Chip {
        kind: ChipKind::Word,
        label: "Word",
    },
    Chip {
        kind: ChipKind::Regex,
        label: ".*",
    },
    Chip {
        kind: ChipKind::HistoryScope,
        label: HISTORY_SCOPE_LABEL,
    },
];

pub(crate) const REPLACE_ROW_CHIPS: [Chip; 2] = [
    Chip {
        kind: ChipKind::ReplaceOne,
        label: "Replace",
    },
    Chip {
        kind: ChipKind::ReplaceAll,
        label: "All",
    },
];

pub(crate) fn chip_text(state: &FindState, chip: Chip) -> Cow<'static, str> {
    match (chip.kind, state.project()) {
        (ChipKind::ReplaceAll, Some(project)) => {
            Cow::Owned(format!("All {}", project.results.len()))
        }
        _ => Cow::Borrowed(chip.label),
    }
}

pub(crate) fn chip_chord(chip: Chip) -> String {
    label_for(chip_command(chip.kind))
}

#[derive(Clone, Copy, Debug)]
pub struct FindPanelGeometry {
    pub outer: Rect,
    pub(crate) frame: Rect,
    pub(crate) find_field: Rect,
    pub(crate) replace_field: Option<Rect>,
    pub(crate) include_field: Option<Rect>,
    pub(crate) exclude_field: Option<Rect>,
    pub(crate) chips: [Option<(Chip, Rect)>; 7],
}

pub(crate) fn carve(main_area: Rect, state: &FindState) -> (Rect, Option<FindPanelGeometry>) {
    let room = main_area.height.saturating_sub(1);
    let globs_wanted = state.globs().is_some();
    let replace_wanted = state.replace.is_some();
    let Some((replace_shown, globs_shown)) = [
        (replace_wanted, globs_wanted),
        (false, globs_wanted),
        (false, false),
    ]
    .into_iter()
    .find(|&(replace, globs)| room >= panel_rows(replace, globs)) else {
        return (main_area, None);
    };
    let height = panel_rows(replace_shown, globs_shown);
    let outer = Region::carve_bottom(main_area, height).rect();
    let rest = Rect::new(
        main_area.x,
        main_area.y,
        main_area.width,
        main_area.height.saturating_sub(height),
    );

    let strip_w = chip_strip_width(replace_shown, state);
    let chips_fit = outer.width >= strip_w.saturating_add(FIND_PANEL_MIN_BOX_W);
    let frame = if chips_fit {
        Rect::new(
            outer.x,
            outer.y,
            outer.width.saturating_sub(strip_w),
            outer.height,
        )
    } else {
        outer
    };
    let find_field = Rect::new(
        frame.x.saturating_add(1),
        frame.y.saturating_add(1),
        frame.width.saturating_sub(2),
        1,
    );
    let replace_field = replace_shown.then(|| {
        Rect::new(
            frame.x.saturating_add(1),
            frame.y.saturating_add(3),
            frame.width.saturating_sub(2),
            1,
        )
    });

    let (include_field, exclude_field) = if globs_shown {
        let (include, exclude) =
            split_globs_row(frame, outer.y.saturating_add(height).saturating_sub(2));
        (Some(include), Some(exclude))
    } else {
        (None, None)
    };

    let mut chips: [Option<(Chip, Rect)>; 7] = [None; 7];
    if chips_fit {
        let x0 = frame.right().saturating_add(1);
        let mut slots = chips.iter_mut();
        lay_chip_row(&mut slots, &FIND_ROW_CHIPS, x0, find_field.y, state);
        if let Some(replace_field) = replace_field {
            lay_chip_row(&mut slots, &REPLACE_ROW_CHIPS, x0, replace_field.y, state);
        }
    }

    (
        rest,
        Some(FindPanelGeometry {
            outer,
            frame,
            find_field,
            replace_field,
            include_field,
            exclude_field,
            chips,
        }),
    )
}

fn panel_rows(replace: bool, globs: bool) -> u16 {
    let base = if replace {
        WITH_REPLACE_ROWS
    } else {
        FIND_ONLY_ROWS
    };
    base.saturating_add(if globs { GLOBS_ROW } else { 0 })
}

fn split_globs_row(frame: Rect, y: u16) -> (Rect, Rect) {
    let inner = frame.width.saturating_sub(2);
    let include_w = inner.saturating_sub(GLOB_GAP) / 2;
    let exclude_w = inner.saturating_sub(GLOB_GAP).saturating_sub(include_w);
    let x = frame.x.saturating_add(1);
    let exclude_x = x.saturating_add(include_w).saturating_add(GLOB_GAP);
    (
        Rect::new(x, y, include_w, 1),
        Rect::new(exclude_x, y, exclude_w, 1),
    )
}

fn lay_chip_row<'a>(
    slots: &mut impl Iterator<Item = &'a mut Option<(Chip, Rect)>>,
    row: &[Chip],
    x0: u16,
    y: u16,
    state: &FindState,
) {
    let mut x = x0;
    for chip in row {
        let width = chip_width(state, *chip);
        if let Some(slot) = slots.next() {
            *slot = Some((*chip, Rect::new(x, y, width, 1)));
        }
        x = x.saturating_add(width).saturating_add(1);
    }
}

fn chip_width(state: &FindState, chip: Chip) -> u16 {
    cells(&chip_chord(chip))
        .saturating_add(1)
        .saturating_add(cells(&chip_text(state, chip)))
}

fn chip_row_width(row: &[Chip], state: &FindState) -> u16 {
    let widths = row
        .iter()
        .map(|chip| chip_width(state, *chip))
        .fold(0u16, u16::saturating_add);
    let gaps = u16::try_from(row.len().saturating_sub(1)).unwrap_or(u16::MAX);
    widths.saturating_add(gaps)
}

fn chip_strip_width(replace_shown: bool, state: &FindState) -> u16 {
    let find_row = chip_row_width(&FIND_ROW_CHIPS, state);
    let widest = if replace_shown {
        find_row.max(chip_row_width(&REPLACE_ROW_CHIPS, state))
    } else {
        find_row
    };
    widest.saturating_add(1)
}

fn cells(label: &str) -> u16 {
    u16::try_from(display_width(label)).unwrap_or(u16::MAX)
}
