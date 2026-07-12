package workspace

import "testing"

// TestActionKindCasts pins actionKind's iota order to the int values the fuzz
// snapshot's PendingDataLossKind field (and the plan's SAVE-NOMUT/GUARD-*
// invariants) assume: None=0, Close=1, Quit=2, Evict=3, Trash=4. A reordering
// of the actionKind const block would silently break every invariant that
// reads snapshot.Snapshot.PendingDataLossKind without this test catching it.
func TestActionKindCasts(t *testing.T) {
	cases := []struct {
		name string
		kind actionKind
		want int
	}{
		{"None", actionNone, 0},
		{"Close", actionClose, 1},
		{"Quit", actionQuit, 2},
		{"Evict", actionEvict, 3},
		{"Trash", actionTrash, 4},
	}
	for _, c := range cases {
		if got := int(c.kind); got != c.want {
			t.Errorf("actionKind %s = %d, want %d", c.name, got, c.want)
		}
	}
}

// TestMergeIntentCasts pins mergeIntent's iota order to the int values
// internal/fuzz/driver/driver_verbatim.go's checkResolveProbe assumes
// (mergeIntentMerge=0, mergeIntentDiscard=1). resolveProbeMsg is unexported,
// so the driver reads its intent field via reflection and mirrors these
// constants by value — a reorder of the mergeIntent const block would
// silently swap DISCARD-ADOPT and MERGE-RESOLVE-ADOPT's routing without
// this test catching it.
func TestMergeIntentCasts(t *testing.T) {
	if got := int(mergeIntentMerge); got != 0 {
		t.Errorf("mergeIntentMerge = %d, want 0", got)
	}
	if got := int(mergeIntentDiscard); got != 1 {
		t.Errorf("mergeIntentDiscard = %d, want 1", got)
	}
}
