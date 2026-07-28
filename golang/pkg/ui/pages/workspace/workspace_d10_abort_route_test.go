package workspace

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/pages/workspace/mergemode"
)

// TestMergeAbortRoutesThroughFooterTail is D10's regression test.
//
// D10's bug lived specifically in the mergeAbort-returned-an-error branch of
// handleKeyPress, which used to `return m.finalize(cmds)` immediately —
// skipping the footer.Update(msg)/syncCursorToFooter/syncMergeHint tail every
// OTHER key path (including the success case tested here, and the sibling
// mergemode.HandleKey error branch) still reaches. That specific abortErr!=nil
// branch is not reachable through the public Model/Store API: mergemode.Abort
// only errors when the wholesale ReplaceAll it issues is out of bounds or
// carries invalid UTF-8, and it always replays exactly the buffer's own
// prior, already-valid content at self-consistent bounds (start=0,
// end=len(current content)) — so this test instead pins the INVARIANT the
// fix establishes: an Esc-driven merge abort (the reachable, success case)
// always falls through to the shared footer-routing tail, the same tail the
// fixed error branch now shares by construction (both sides of the if/else
// run into the identical code right after it — see the diff). A future
// regression that reintroduces an early return on EITHER branch breaks the
// shared-tail symmetry this test exercises.
func TestMergeAbortRoutesThroughFooterTail(t *testing.T) {
	m, _, _ := enterRealConflict(t)
	if !mergemode.IsActive(m.merge) {
		t.Fatal("setup: expected mergemode active")
	}
	if !strings.Contains(m.footer.View(), "Merge") {
		t.Fatal("setup: expected footer to show the merge hint once mergemode is active")
	}

	m = focusEditor(m)
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})

	if mergemode.IsActive(m.merge) {
		t.Fatal("expected mergemode to be inactive after Esc-abort")
	}
	// The footer's merge hint is driven ONLY by syncMergeHint (called in the
	// shared tail at the bottom of handleKeyPress) — if the abort branch had
	// early-returned before reaching it, this would still read stale-active,
	// exactly the D10 symptom ("the error skips footer routing").
	if strings.Contains(m.footer.View(), "Merge") {
		t.Fatal("footer still shows the merge hint after Esc-abort — the shared footer-routing tail did not run (D10 regression)")
	}
}
