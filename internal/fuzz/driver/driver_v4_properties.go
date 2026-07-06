//go:build fuzzing

package driver

import (
	"database/sql"
	"errors"
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/session"
	"rune/internal/fuzz/snapshot"
	"rune/pkg/docstate"
	pgworkspace "rune/pkg/ui/pages/workspace"
)

// checkV4Properties runs the three Data-Integrity Model v4 fuzz properties
// (Part V WP7) against the message/snapshot drainMsg just settled. Extracted
// from drainMsg to keep driver.go under the 500-LoC limit (§1.6/§11) — pure
// extraction, no behavior change; called inline from drainMsg with the exact
// same rs/msg/snap it already has in scope.
func checkV4Properties(rs *runState, msg tea.Msg, snap snapshot.Snapshot) *invariant.Violation {
	// ---- WP7 property (b): store.Content(doc) == live buffer, EVERY step --
	// Broader than DL1 (drainMsg's own AutosaveSettledMsg-gated check):
	// AppendEdit commits synchronously within the same Update call that
	// applied the edit, so RecoverDocument's replay should track the live
	// buffer at every settled step, not just after a debounced autosave
	// flush. Reuses the same DL1 comparison (CheckDataLoss) — this is the
	// identical property, checked at a strictly wider set of points. Checked
	// at EVERY message type, including FileLoadedMsg (data-integrity-v4
	// remediation WP-R2 item 3): a SyncDiskAhead reopen now installs theirs
	// via a REAL journaled adoption (F1 — handleFileLoadedMsg's
	// installDiskAhead), so store.Content(doc) tracks the displayed buffer
	// at the FileLoadedMsg step itself, not just afterward. The old
	// exclusion assumed the pre-fix bare-SetContent behavior was intended;
	// with the fix it would mask exactly the class of bug F1 closes.
	// Deterministic cost bound: a full reconstruction per step is quadratic
	// in journal length, and pathological long inputs (~300 ops) exceeded
	// the fuzz coordinator's per-exec hang threshold (>5s per exec, killed
	// as flaky "hung or terminated unexpectedly"). Every step is checked for
	// the first 64; afterwards every 8th step — EXCEPT the transition
	// messages this property exists to police (load installs / save acks /
	// autosave settles), which are ALWAYS checked regardless of sampling:
	// the F1/DL1 bug class lives exactly at those points, so sampling never
	// weakens their detection, only thins the steady-typing steps between.
	// AutosaveSettledMsg is deliberately NOT in the always-check list: under
	// fuzzing flushDelay is 0, so it fires per keystroke — and drainMsg's own
	// DL1 check (driver.go) already runs on exactly that message; a second
	// unconditional reconstruction here would just double the per-keystroke
	// cgo/SQLite cost that made long inputs trip the worker-hang kill.
	rs.steps++
	alwaysCheck := false
	switch msg.(type) {
	case pgworkspace.FileLoadedMsg, pgworkspace.FileSavedMsg:
		alwaysCheck = true
	}
	if rs.store != nil && snap.DocID != 0 && (alwaysCheck || rs.steps <= 64 || rs.steps%8 == 0) {
		if vfsContent, err := rs.store.Content(snap.DocID); err == nil {
			if v := session.CheckDataLoss(snap, vfsContent); v != nil {
				return v
			}
		}
	}

	// ---- WP7 property (a): capture-before-discard, driver-observable half -
	// Right after OUR OWN write replaces disk content (a FileSavedMsg lands —
	// "at replacement time"), the observation Materialize itself just
	// recorded (FileSavedMsg.Result.Saved, populated the moment commitSave's
	// own PutBlob committed — §1.4.10) must have a corresponding blob.
	// Checked against msg.Result.Saved.BlobHash specifically, NOT a fresh
	// Mem-disk read: RunHuman/RunReorderSaves/RunDelayedViewResult all also
	// perform RAW external mem.WriteFile calls (simulating another editor,
	// or — under RunReorderSaves' deferred-delivery — a LATER save landing
	// before this message is finally delivered) — disk can legitimately hold
	// DIFFERENT bytes than what THIS save wrote by the time its message is
	// processed. I1 is about what WE overwrite at the moment we overwrite
	// it, not about disk's staleness relative to a delayed message delivery
	// or an unobserved external write — reading the recorded observation
	// directly (rather than re-deriving it from current disk state) is
	// immune to both.
	if savedMsg, ok := msg.(pgworkspace.FileSavedMsg); ok && rs.store != nil && savedMsg.Result.Committed {
		// GetBlob can fail two structurally different ways: the blob is
		// genuinely absent (sql.ErrNoRows — a real I1 violation), or the
		// underlying connection is gone (teardownAndQuit's store.Close()
		// runs SYNCHRONOUSLY inside the very Update() call that produces
		// THIS FileSavedMsg — cluster 15/quitSaveAll's saveLeft==0 branch —
		// one full message before snap.AppQuitting is ever observably true,
		// so there is no earlier gate to check). Only the former is
		// flagged; the latter means "cannot verify right now", not "I1
		// failed" — skip, don't false-positive (matches every other
		// driver-level store read's error discipline, e.g. mirrorFor).
		if _, err := rs.store.GetBlob(savedMsg.Result.Saved.BlobHash); err != nil && errors.Is(err, sql.ErrNoRows) {
			return &invariant.Violation{
				InvariantID: "CAPTURE-BEFORE-DISCARD",
				Message: fmt.Sprintf("save for %s recorded observation with blob_hash %s, but no corresponding blob exists — I1 violated",
					invariant.Trunc(savedMsg.Path, 60), savedMsg.Result.Saved.BlobHash[:min(12, len(savedMsg.Result.Saved.BlobHash))]),
			}
		}
	}

	// ---- WP7 property (c): DirtyDocs agrees with ground truth -------------
	// store.DirtyDocs' classification for the displayed doc must agree with
	// the Adoption Contract's dirty definition (data-integrity-v4 remediation
	// WP-R1 item 2): dirty ⟺ ours differs from the derived 3-way-merge
	// ancestor (SyncBufferAhead or SyncDiverged) — NEVER a raw "content vs.
	// saved_obs" comparison. That OLD ground truth is now a DIFFERENT fact by
	// design: SyncDiskAhead (disk moved, saved_obs is deliberately left
	// stale — the Adoption Contract, observation.go) has content != the
	// saved_obs blob while nothing of the user's is actually unsaved, so a
	// raw content/saved_obs diff would flag it dirty and fail this property
	// against CORRECT new behavior. Checked against Sync(docID) directly
	// (not IsDirty/DirtyDocs' own internals) — an independent cross-check
	// that the two exported entry points classify the SAME document
	// identically.
	if rs.store != nil && snap.DocID != 0 {
		if sync, sErr := rs.store.Sync(snap.DocID); sErr == nil {
			if dirtyMap, dErr := rs.store.DirtyDocs([]int64{snap.DocID}); dErr == nil {
				groundTruthDirty := sync.Kind == docstate.SyncBufferAhead || sync.Kind == docstate.SyncDiverged
				if dirtyMap[snap.DocID] != groundTruthDirty {
					return &invariant.Violation{
						InvariantID: "DIRTYDOCS-GROUND-TRUTH",
						Message: fmt.Sprintf("DirtyDocs[%d]=%v, ground truth (Sync.Kind=%v)=%v",
							snap.DocID, dirtyMap[snap.DocID], sync.Kind, groundTruthDirty),
					}
				}
			}
		}
	}

	return nil
}
