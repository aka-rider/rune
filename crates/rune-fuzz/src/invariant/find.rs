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

fn types_into_a_field(input: KeyInput) -> bool {
    matches!(input.code, KeyCode::Char(c) if !c.is_control())
        && !input.mods.alt
        && !input.mods.ctrl
        && !input.mods.sup
}
