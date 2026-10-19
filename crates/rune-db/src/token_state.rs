use std::collections::HashMap;
use std::sync::Arc;

use rune_core::assert_invariant;

use crate::history::Lineage;
use crate::ids::{BindingToken, DocId, Seq};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenOrigin {
    base: Seq,
    history: Arc<[Seq]>,
    pos: usize,
}

impl TokenOrigin {
    pub fn at(base: Seq) -> TokenOrigin {
        TokenOrigin {
            base,
            history: Arc::from([]),
            pos: 0,
        }
    }

    pub fn from_lineage(lineage: &Lineage) -> TokenOrigin {
        TokenOrigin {
            base: lineage.base_seq,
            history: lineage.seqs().collect(),
            pos: lineage.done.len(),
        }
    }
}

/// This writer thread's own record of one [`BindingToken`]'s LOCAL
/// undo-position numbering, scoped to THIS process's session (never shared
/// or persisted). A token is minted fresh by the app on every bind/rebind
/// (`rune-tui`'s `DocDb::new`), so this map starts every token from its
/// [`TokenOrigin`] — two tokens sharing one [`DocId`] (unreachable via any
/// real open path, but not structurally prevented) each get their own
/// independent entry rather than racing to fill one shared sequence.
/// `base_seq` is the durable seq local position `0` resolves to;
/// `local_seq[i]` is the durable seq local position `i + 1` resolves to —
/// first the origin's stored history, then each `AppendEdit` THIS writer
/// thread has run for this token, in the order they ran. Rebuilding this
/// table from ops this thread has ALREADY executed — rather than trusting a
/// value carried in from the app, which can only know an op's outcome once
/// its ack has round-tripped — is what makes `OpKind::MoveUndoPos`'s
/// resolution exact instead of a guess at an in-flight ack.
pub(crate) struct DocUndoState {
    pub(crate) base_seq: Seq,
    pub(crate) local_seq: Vec<Seq>,
    pub(crate) pos: usize,
}

impl DocUndoState {
    fn seeded(origin: &TokenOrigin) -> DocUndoState {
        DocUndoState {
            base_seq: origin.base,
            local_seq: origin.history.to_vec(),
            pos: origin.pos,
        }
    }

    /// The durable seq LOCAL undo position `local_pos` resolves to, or
    /// `None` if this state has no entry for it — `local_pos` is deeper than
    /// any `AppendEdit` this thread has actually run for this token, an
    /// invariant violation the writer's single FIFO queue should make
    /// unreachable (see `OpKind::MoveUndoPos`'s doc comment) — never
    /// silently approximated.
    pub(crate) fn resolve(&mut self, local_pos: i64) -> Option<Seq> {
        if local_pos == 0 {
            self.pos = 0;
            return Some(self.base_seq);
        }
        let idx = usize::try_from(local_pos - 1).ok()?;
        let seq = *self.local_seq.get(idx)?;
        self.pos = idx + 1;
        Some(seq)
    }

    pub(crate) fn push_seq(&mut self, doc_id: DocId, seq: Seq) {
        self.local_seq.truncate(self.pos);
        assert_invariant!(self.local_seq.last().is_none_or(|&last| seq > last), || {
            format!(
                "append_edit doc {doc_id}: seq {seq} did not advance past local_seq.last() {:?}",
                self.local_seq.last()
            )
        });
        self.local_seq.push(seq);
        self.pos = self.local_seq.len();
    }
}

/// Looks up `token`'s [`DocUndoState`], seeding it from `origin` on first
/// sight — a token's numbering starts the instant its first op reaches the
/// front of the writer's queue, whichever op that is.
pub(crate) fn ensure_token_state<'a>(
    undo_state: &'a mut HashMap<BindingToken, DocUndoState>,
    token: BindingToken,
    origin: &TokenOrigin,
) -> &'a mut DocUndoState {
    undo_state
        .entry(token)
        .or_insert_with(|| DocUndoState::seeded(origin))
}

#[cfg(test)]
#[path = "token_state_tests.rs"]
mod tests;
