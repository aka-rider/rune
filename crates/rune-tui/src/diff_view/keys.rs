use rune_core::buffer::Edit;
use rune_core::cursor::{CursorId, CursorSet};
use rune_core::undo::EditKind;
use rune_merge::RegionKind;
use std::ops::Range;

use crate::app::App;
use crate::binding::{Binding, KeyPattern, resolve_in};
use crate::commands::edit_core::apply_edit_batch_with_cursors;
use crate::commands::nav_scroll;
use crate::document::DocumentId;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::messages;

use super::DiffView;
use super::rows::line_offset;

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};

const SUP_SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: true,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffCommand {
    NextHunk,
    PrevHunk,
    TakeTheirs,
    TakeOurs,
}

pub const DIFF_BINDINGS: &[Binding<DiffCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Char('j'), SUP_SHIFT),
        cmd: DiffCommand::NextHunk,
        help: "next hunk",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('J'), SUP),
        cmd: DiffCommand::NextHunk,
        help: "next hunk",
        alias: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('k'), CTRL),
        cmd: DiffCommand::PrevHunk,
        help: "prev hunk",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('y'), SUP_SHIFT),
        cmd: DiffCommand::TakeTheirs,
        help: "take theirs",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('Y'), SUP),
        cmd: DiffCommand::TakeTheirs,
        help: "take theirs",
        alias: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('u'), SUP_SHIFT),
        cmd: DiffCommand::TakeOurs,
        help: "take ours",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('U'), SUP),
        cmd: DiffCommand::TakeOurs,
        help: "take ours",
        alias: true,
    },
];

pub(crate) fn intercept(app: &mut App, key: KeyInput) -> bool {
    let Some(diff) = app.diff.as_ref() else {
        return false;
    };
    if diff.right != app.active {
        return false;
    }
    let merge_active = crate::merge::verbs::active_on(app, app.active);
    if merge_active && key.code == KeyCode::Escape && key.mods == Mods::NONE {
        crate::merge::exit_in_place(app);
        return true;
    }
    let Some(cmd) = resolve_in(DIFF_BINDINGS, key) else {
        return false;
    };
    match (cmd, merge_active) {
        (DiffCommand::NextHunk, true) => crate::merge::verbs::nav(app, 1),
        (DiffCommand::PrevHunk, true) => crate::merge::verbs::nav(app, -1),
        (DiffCommand::TakeTheirs, true) => crate::merge::verbs::take_theirs(app),
        (DiffCommand::TakeOurs, true) => crate::merge::verbs::take_ours(app),
        (DiffCommand::NextHunk, false) => move_hunk(app, 1),
        (DiffCommand::PrevHunk, false) => move_hunk(app, -1),
        (DiffCommand::TakeTheirs, false) => take_theirs(app),
        (DiffCommand::TakeOurs, false) => take_ours(app),
    }
    true
}

fn hunk_indices(diff: &DiffView) -> Vec<usize> {
    diff.alignment
        .regions
        .iter()
        .enumerate()
        .filter(|(_, region)| region.kind != RegionKind::Same)
        .map(|(idx, _)| idx)
        .collect()
}

fn current_hunk_index(diff: &DiffView) -> Option<usize> {
    let hunks = hunk_indices(diff);
    if hunks.is_empty() {
        return None;
    }
    Some(if hunks.contains(&diff.hunk_cur) {
        diff.hunk_cur
    } else {
        hunks.first().copied().unwrap_or(diff.hunk_cur)
    })
}

fn move_hunk(app: &mut App, dir: isize) {
    let Some(diff) = app.diff.as_ref() else {
        return;
    };
    let hunks = hunk_indices(diff);
    if hunks.is_empty() {
        messages::info(app, "no hunks");
        return;
    }
    let cur_pos = hunks.iter().position(|&idx| idx == diff.hunk_cur);
    let len = hunks.len() as isize;
    let next_pos = match cur_pos {
        Some(pos) => (((pos as isize + dir) % len + len) % len) as usize,
        None if dir >= 0 => 0,
        None => hunks.len() - 1,
    };
    let Some(&region_idx) = hunks.get(next_pos) else {
        return;
    };
    let right = diff.right;
    let Some(region) = diff.alignment.regions.get(region_idx) else {
        return;
    };
    let target_line = region.right_lines.start;
    let ordinal = next_pos + 1;
    let total = hunks.len();

    if let Some(diff) = app.diff.as_mut() {
        diff.hunk_cur = region_idx;
    }
    if let Some(doc) = app.doc(right) {
        let byte = line_offset(&doc.buffer, target_line);
        if let Some(doc) = app.doc_mut(right) {
            nav_scroll::scroll_to_byte_offset(doc, byte);
        }
    }
    messages::info(app, format!("hunk {ordinal}/{total}"));
}

enum HunkReplacement {
    None,
    RangeInvalid,
    Some(DocumentId, Range<usize>, String),
}

fn current_hunk_replacement(app: &App) -> HunkReplacement {
    let Some(diff) = app.diff.as_ref() else {
        return HunkReplacement::None;
    };
    let Some(region_idx) = current_hunk_index(diff) else {
        return HunkReplacement::None;
    };
    let Some(region) = diff.alignment.regions.get(region_idx) else {
        return HunkReplacement::None;
    };
    let right_lines = region.right_lines.clone();
    let left_lines = region.left_lines.clone();
    let left_start = line_offset(&diff.left.buffer, left_lines.start);
    let left_end = line_offset(&diff.left.buffer, left_lines.end);
    let Some(insert) = diff.left.buffer.content().get(left_start..left_end) else {
        return HunkReplacement::RangeInvalid;
    };
    HunkReplacement::Some(diff.right, right_lines, insert.to_string())
}

fn take_theirs(app: &mut App) {
    let (right, right_lines, insert) = match current_hunk_replacement(app) {
        HunkReplacement::None => {
            messages::info(app, "no hunk to take");
            return;
        }
        HunkReplacement::RangeInvalid => {
            messages::error(app, "merge: the hunk range no longer matches the pane text");
            return;
        }
        HunkReplacement::Some(right, right_lines, insert) => (right, right_lines, insert),
    };
    let Some(right_doc) = app.doc(right) else {
        return;
    };
    let right_start = line_offset(&right_doc.buffer, right_lines.start);
    let right_end = line_offset(&right_doc.buffer, right_lines.end);
    let cursors_before = right_doc.cursors.clone();
    let edit = Edit {
        start: right_start,
        end: right_end,
        insert,
    };
    let applied = apply_edit_batch_with_cursors(
        app,
        right,
        vec![(edit, CursorId::FIRST)],
        &cursors_before,
        EditKind::Other,
        move |_, _| vec![CursorSet::new(right_start).primary()],
    );
    if applied {
        messages::info(app, "took theirs");
    } else {
        messages::warn(app, "hunk could not be applied");
    }
}

fn take_ours(app: &mut App) {
    messages::info(app, "already yours");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global::GLOBAL_BINDINGS;

    #[test]
    fn diff_bindings_are_not_claimed_by_the_global_table() {
        for binding in DIFF_BINDINGS {
            let pattern = binding.key;
            let key = match pattern.key {
                crate::binding::KeyMatch::Code(code) => KeyInput {
                    code,
                    mods: pattern.mods,
                },
                crate::binding::KeyMatch::Printable => continue,
            };
            let claimants: Vec<&'static str> = GLOBAL_BINDINGS
                .iter()
                .filter(|b| b.key.matches(key))
                .map(|b| b.help)
                .collect();
            assert!(
                claimants.is_empty(),
                "GLOBAL_BINDINGS would shadow diff key {key:?}: {claimants:?}"
            );
        }
    }
}
