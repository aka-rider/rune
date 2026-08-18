use rune_tui::focus::FocusTarget;

use super::Violation;
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

pub fn palette_focus_stable(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if prev.focus_target != FocusTarget::Palette || next.focus_target != FocusTarget::Palette {
        return None;
    }
    if !matches!(ctx.msg, MsgTag::Key { .. }) {
        return None;
    }
    if next.focus != prev.focus {
        return Some(Violation::new(
            "PALETTE-FOCUS-STABLE",
            format!(
                "palette is open but focus moved from {:?} to {:?}",
                prev.focus, next.focus
            ),
        ));
    }
    None
}

pub fn palette_guard(next: &Snapshot) -> Option<Violation> {
    if next.guard.is_some() && next.focus_target == FocusTarget::Palette {
        return Some(Violation::new(
            "PALETTE-GUARD",
            "a Guard is raised while the palette overlay is still open".to_string(),
        ));
    }
    None
}
