use rune_tui::focus::FocusTarget;
use rune_tui::keymap::{KeyCode, KeyInput};

use super::Violation;
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

pub fn find_preview_pure(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    let MsgTag::Key { input, .. } = &ctx.msg else {
        return None;
    };
    if !types_into_a_field(*input) || !prev.replace_field_focused || !next.replace_field_focused {
        return None;
    }
    let (doc, before, after) = prev.version_by_doc.iter().find_map(|(doc, before)| {
        let after = next.version_by_doc.get(doc)?;
        (after != before).then_some((*doc, *before, *after))
    })?;
    Some(Violation::new(
        "FIND-PREVIEW-PURE",
        format!(
            "typing {:?} into the Replace field edited document {doc:?}: buffer version \
             {before} -> {after}",
            input.code
        ),
    ))
}

pub fn find_replace_no_disk(prev: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    let MsgTag::Key { input, .. } = &ctx.msg else {
        return None;
    };
    if !is_replace_all_key(*input) || prev.focus_target != FocusTarget::Find {
        return None;
    }
    if ctx.disk_before == ctx.disk {
        return None;
    }
    Some(Violation::new(
        "FIND-REPLACE-NO-DISK",
        format!(
            "replace-all in the find panel changed the file on disk: {} -> {} bytes",
            ctx.disk_before.as_ref().map_or(0, Vec::len),
            ctx.disk.as_ref().map_or(0, Vec::len)
        ),
    ))
}

pub fn find_walk_drains(next: &Snapshot) -> Option<Violation> {
    if next.find_walk_queued != Some(0) {
        return None;
    }
    Some(Violation::new(
        "FIND-WALK-DRAINS",
        "a project replace walk with nothing queued survived the step instead of finishing"
            .to_string(),
    ))
}

fn types_into_a_field(input: KeyInput) -> bool {
    matches!(input.code, KeyCode::Char(c) if !c.is_control())
        && !input.mods.alt
        && !input.mods.ctrl
        && !input.mods.sup
}

fn is_replace_all_key(input: KeyInput) -> bool {
    input.code == KeyCode::Enter
        && input.mods.shift
        && !input.mods.alt
        && !input.mods.ctrl
        && !input.mods.sup
}
