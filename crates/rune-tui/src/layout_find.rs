use std::borrow::Cow;

use ratatui::layout::Rect;

use crate::find::{Control, FindState};
use crate::region::Region;
use crate::width::display_width;

pub const FIND_PANEL_MIN_BOX_W: u16 = 12;
const FIND_ONLY_ROWS: u16 = 3;
const WITH_REPLACE_ROWS: u16 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Chip {
    pub control: Control,
    pub label: &'static str,
}

pub(crate) const FIND_ROW_CHIPS: [Chip; 4] = [
    Chip {
        control: Control::Scope,
        label: "[File|Project]",
    },
    Chip {
        control: Control::Case,
        label: "[Aa]",
    },
    Chip {
        control: Control::Word,
        label: "[Word]",
    },
    Chip {
        control: Control::Regex,
        label: "[.*]",
    },
];

pub(crate) const REPLACE_ROW_CHIPS: [Chip; 2] = [
    Chip {
        control: Control::ReplaceOne,
        label: "[Replace]",
    },
    Chip {
        control: Control::ReplaceAll,
        label: "[All]",
    },
];

pub(crate) fn chip_label(state: &FindState, chip: Chip) -> Cow<'static, str> {
    match (chip.control, &state.project) {
        (Control::ReplaceAll, Some(project)) => {
            Cow::Owned(format!("[All {}]", project.results.len()))
        }
        _ => Cow::Borrowed(chip.label),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FindPanelGeometry {
    pub outer: Rect,
    pub(crate) frame: Rect,
    pub(crate) find_field: Rect,
    pub(crate) replace_field: Option<Rect>,
    pub(crate) chips: [Option<(Chip, Rect)>; 6],
}

pub(crate) fn carve(main_area: Rect, state: &FindState) -> (Rect, Option<FindPanelGeometry>) {
    let room = main_area.height.saturating_sub(1);
    let height = if state.replace.is_some() && room >= WITH_REPLACE_ROWS {
        WITH_REPLACE_ROWS
    } else if room >= FIND_ONLY_ROWS {
        FIND_ONLY_ROWS
    } else {
        return (main_area, None);
    };
    let outer = Region::carve_bottom(main_area, height).rect();
    let rest = Rect::new(
        main_area.x,
        main_area.y,
        main_area.width,
        main_area.height.saturating_sub(height),
    );
    let replace_shown = height == WITH_REPLACE_ROWS;

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

    let mut chips: [Option<(Chip, Rect)>; 6] = [None; 6];
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
            chips,
        }),
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
        let width = cells(&chip_label(state, *chip));
        if let Some(slot) = slots.next() {
            *slot = Some((*chip, Rect::new(x, y, width, 1)));
        }
        x = x.saturating_add(width).saturating_add(1);
    }
}

fn chip_row_width(row: &[Chip], state: &FindState) -> u16 {
    let labels: u16 = row
        .iter()
        .map(|chip| cells(&chip_label(state, *chip)))
        .sum();
    let gaps = u16::try_from(row.len().saturating_sub(1)).unwrap_or(u16::MAX);
    labels.saturating_add(gaps)
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
