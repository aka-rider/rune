//go:build fuzzing

package driver

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/snapshot"
	"rune/pkg/editor/buffer"
	"rune/pkg/ui/components/footer"
	pgworkspace "rune/pkg/ui/pages/workspace"
)

// isLoadResult reports whether msg is an async file-load result — the only
// message class RunReorder defers, since out-of-order delivery of these is the
// race under test.
func isLoadResult(msg tea.Msg) bool {
	switch msg.(type) {
	case pgworkspace.FileLoadedMsg, pgworkspace.FileLoadErrorMsg:
		return true
	}
	return false
}

// isLoadFamilyMsg reports whether msg is a load-driven (or equally
// legitimate, explicit-user-confirmed) restore: the doc's baseline is
// legitimately re-established from disk/journal content or purged to blank,
// not from the buffer's own undo-derived state. %T-name matched (the same
// convention internal/fuzz/ui/* uses for workspace's other unexported
// message types) since lateBindLoadedMsg is unexported and driver.go cannot
// import it across the package boundary. prev is the pre-Update snapshot —
// needed to correlate a DataLossGuardResponseMsg to the specific guard it
// resolved.
func isLoadFamilyMsg(msg tea.Msg, prev snapshot.Snapshot) bool {
	if isLoadResult(msg) {
		return true // FileLoadedMsg / FileLoadErrorMsg
	}
	if _, ok := msg.(pgworkspace.StoreReadyMsg); ok {
		return true
	}
	switch fmt.Sprintf("%T", msg) {
	case "workspace.lateBindLoadedMsg":
		// The store-becomes-ready late bind: the startup untitled tab was
		// created store-less (docID==0, mirrorFor is a no-op for it); THIS
		// message is what first assigns its real docID and carries the
		// actual freshly-read disk content — the doc's true first baseline.
		return true
	}
	if resp, ok := msg.(footer.DataLossGuardResponseMsg); ok &&
		resp.Response == footer.DataLossDiscard && prev.PendingDeletedActive {
		// GuardDeleted's [D]iscard purges the doc's history
		// (handleDeletedDiscard → store.DeleteDoc) AND resets the buffer to
		// blank — the same "explicit, prompt-confirmed baseline reset"
		// class CLOSE-NO-LOSS already carves out (driver_verbatim.go), not
		// a buffer that merely got fully undone. Found via this exact
		// scenario false-positiving SHADOW once UNDO-BASELINE landed (WP7
		// session): mirrorFor otherwise kept comparing the fresh blank
		// buffer against the doc's stale pre-purge baseline.
		return true
	}
	return false
}

// mirrorFor reconstructs the document's content independently of the live editor
// buffer: the loaded baseline plus every journaled "main" edit replayed forward.
// The baseline is captured the first time a doc is seen with zero edits (its buffer
// IS the freshly-loaded content then); later edits replay on top — matching how the
// editor builds the buffer (loaded content + edits). This lets SHADOW validate that
// the journal faithfully tracks the buffer for LOADED files, not just untitled ones
// born empty. AllEdits returns the full edit log (not just post-snapshot), so the
// reconstruction is correct across autosave snapshots.
//
// UNDO-BASELINE (WP7, landed last with its own soak per the plan): isLoadFamily
// distinguishes a legitimate fresh baseline (a new/re- load) from a buffer
// that's merely fully undone (ActiveEdits empty because the undo head is back
// at the load position, NOT because a new doc was loaded). Silently
// re-adopting whatever the LIVE BUFFER holds as the new baseline in the
// latter case is a soundness gap: a full-undo restoring WRONG bytes (an undo
// bug) would be blessed as "correct" and never caught again. Verified safe
// per the plan: title edits are never journaled; discard/merge resolutions
// journal a ReplaceAll (batches stays non-empty, this branch doesn't apply);
// an untitled doc starts baselined at "" already.
func (rs *runState) mirrorFor(docID int64, bufferContent string, isLoadFamily bool) string {
	if rs.store == nil || docID == 0 {
		return ""
	}
	// ActiveEdits (not AllEdits) so the mirror honors the undo head (seq <=
	// current_seq): after an undo the buffer drops the undone edits, and the mirror
	// must too, or it diverges (SHADOW). Per-doc now (I2: no more surface
	// dimension) — the docID itself is what used to be filtered by "main".
	batches, err := rs.store.ActiveEdits(docID)
	if err != nil {
		// Skip, don't approximate: a read error here is expected once
		// teardownAndQuit (workspace_quit.go) closes the store mid-run
		// (cluster 15/quitSaveAll) — every subsequent ActiveEdits call on
		// the same doc errors, so falling back to the STALE pre-edit
		// baseline would compare it against a live buffer that has since
		// moved on, false-positiving SHADOW on a doc that was never
		// actually corrupted (the store simply can't confirm it anymore).
		// "" is SHADOW's own established skip signal (its gate is
		// `MirrorContent != ""`) — fire-and-forget: degrades coverage for
		// this doc, never loses data, never false-positives.
		return ""
	}
	if len(batches) == 0 {
		hasHistory, hhErr := rs.store.HasHistory(docID)
		if baseline, exists := rs.baselines[docID]; exists && !isLoadFamily && hhErr == nil && hasHistory {
			// Fully undone (or never edited) on a doc we've already
			// baselined, and this settle was NOT driven by a fresh load —
			// the buffer must still match that baseline. Return it
			// UNCHANGED rather than silently adopting whatever the buffer
			// now holds: SHADOW's own Content != MirrorContent comparison
			// (internal/fuzz/ui/workspace/workspace.go) then catches a
			// genuine divergence — no separate/duplicated check needed.
			//
			// Gated on the STORE's own HasHistory, not just "a baseline
			// entry exists in rs.baselines": docID is a SQLite rowid, and
			// GuardDeleted's [D]iscard purge (store.DeleteDoc) followed by
			// a LATER CreateNewFile can hand that SAME numeric ID to a
			// brand-new, unrelated document — rs.baselines[docID] is then a
			// stale cache entry for a document that, from the STORE's own
			// point of view, no longer has any history at all. Trusting it
			// anyway false-positived SHADOW comparing a fresh blank
			// untitled buffer against the PURGED doc's old content (found
			// via FuzzHumanSession's dirtyCloseGuard-discard →
			// CreateNewFile sequence, WP7 follow-up session).
			return baseline
		}
		// No baseline yet, a load/reload legitimately establishes a new
		// one, or the store itself has no history for this docID (a fresh
		// or ID-reused document) — (re)record it per-doc so subsequent
		// edits replay on top.
		rs.baselines[docID] = bufferContent
		return bufferContent
	}
	return buffer.ReplayForward(rs.baselines[docID], batches)
}
