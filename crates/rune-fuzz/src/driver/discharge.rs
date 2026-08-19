use rune_tui::db::DbBridge;
use rune_tui::runtime::Msg;

use crate::step::MsgTag;

use super::session::State;
use super::store_ops::wait_for_db_op;

pub(super) fn discharge_pending_save(state: &mut State) -> Option<(Msg, MsgTag, Option<Vec<u8>>)> {
    let (cmd, per_doc_bytes) = state.pending_save.take()?;
    let msg = cmd.run()?;
    match &msg {
        Msg::SaveDone {
            id,
            version,
            result,
            ..
        } => {
            let tag = MsgTag::SaveDone {
                id: *id,
                version: *version,
                ok: result.is_ok(),
            };
            let bytes = per_doc_bytes.get(id).cloned().unwrap_or_default();
            Some((msg, tag, Some(bytes)))
        }
        Msg::MaterializeVfsDone { id, outcome, .. } => {
            let committed = matches!(
                outcome,
                rune_tui::materialize_ack::MaterializeVfsOutcome::Committed { .. }
                    | rune_tui::materialize_ack::MaterializeVfsOutcome::Raced { .. }
            );
            let tag = MsgTag::MaterializeVfsDone { id: *id, committed };
            Some((msg, tag, None))
        }
        _ => None,
    }
}

pub(super) fn discharge_pending_rename(state: &mut State) -> Option<(Msg, MsgTag)> {
    let cmd = state.pending_rename.take()?;
    let msg = cmd.run()?;
    if !matches!(msg, Msg::RenameDone { .. }) {
        return None;
    }
    Some((msg, MsgTag::RenameDone))
}

pub(super) fn drain_one_db_op(state: &mut State, bridge: &DbBridge) -> Option<(Msg, MsgTag)> {
    let op_id = *state.app.db_ops.keys().min()?;
    let doc = state.app.db_ops.get(&op_id).map(|pending| pending.doc);
    let evt = wait_for_db_op(bridge, op_id);
    let save_committed = matches!(
        &evt,
        rune_db::DbEvent::Ok {
            result: rune_db::OpOutcome::Materialize(mat),
            ..
        } if matches!(
            mat.as_ref(),
            rune_db::MatResult::Committed { .. } | rune_db::MatResult::CommittedRaced { .. }
        )
    );
    Some((
        Msg::Db(evt),
        MsgTag::Db {
            op_id,
            doc,
            save_committed,
        },
    ))
}

pub(super) fn discharge_pending_trash(state: &mut State) -> Option<(Msg, MsgTag)> {
    let cmd = state.pending_trash.take()?;
    let msg = cmd.run()?;
    if !matches!(msg, Msg::TrashDone { .. }) {
        return None;
    }
    Some((msg, MsgTag::TrashDone))
}

fn highlight_span_count(result: &rune_tui::highlight::PassOutcome) -> usize {
    let rune_tui::highlight::PassOutcome::Replace(reply) = result else {
        return 0;
    };
    reply
        .regions
        .iter()
        .map(|region| match &region.outcome {
            rune_tui::highlight::RegionOutcome::Replace(
                rune_tui::highlight::RegionPayload::Spans { spans, .. },
            ) => spans.len(),
            _ => 0,
        })
        .sum()
}

pub(super) fn discharge_pending_highlight(state: &mut State) -> Option<(Msg, MsgTag)> {
    let cmd = state.pending_highlights.pop_front()?;
    let msg = cmd.run()?;
    let Msg::Highlighted {
        version, result, ..
    } = &msg
    else {
        return None;
    };
    let tag = MsgTag::Highlighted {
        delivered_version: *version,
        span_count: highlight_span_count(result),
    };
    Some((msg, tag))
}
