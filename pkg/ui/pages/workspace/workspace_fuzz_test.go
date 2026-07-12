package workspace

import "testing"

// TestFuzzLegacyPendingKind_LegacyIotaOrder pins fuzzLegacyPendingKind's
// legacy iota mapping (None=0, Close=1, Quit=2, Evict=3 — Trash=4 is covered
// separately below, tied to guard.kind rather than an intent's .active bit)
// that the fuzz snapshot's PendingDataLossKind field (and the plan's
// SAVE-NOMUT/GUARD-* invariants) assume. Pre-A4 this pinned actionKind's own
// iota order directly; A4 deleted actionKind (folded into guardState), so
// this now drives fuzzLegacyPendingKind through the guard.close/quit/evict
// intents themselves — each independently of guard.kind (critic R1: a
// conflict guard raised mid-close/evict/quit-save leaves guard.kind reading
// guardConflict while the intent's own .active bit stays true), exactly
// mirroring how pendingDataLoss.kind was independent of guard.kind pre-A4.
func TestFuzzLegacyPendingKind_LegacyIotaOrder(t *testing.T) {
	cases := []struct {
		name string
		set  func(*Model)
		want int
	}{
		{"None", func(m *Model) {}, 0},
		{"Close", func(m *Model) { m.guard.close = closeIntent{active: true} }, 1},
		{"Quit", func(m *Model) { m.guard.quit = quitIntent{active: true} }, 2},
		{"Evict", func(m *Model) { m.guard.evict = evictIntent{active: true} }, 3},
	}
	for _, c := range cases {
		var m Model
		c.set(&m)
		if got := m.fuzzLegacyPendingKind(); got != c.want {
			t.Errorf("fuzzLegacyPendingKind() with %s intent active = %d, want %d", c.name, got, c.want)
		}
	}
}

// TestFuzzLegacyPendingKind_Trash pins fuzzLegacyPendingKind's Trash mapping:
// a Model with guard.kind==guardTrash must report legacy value 4 — the exact
// slot actionTrash held before A3 removed it from actionKind — so
// internal/fuzz/ui/workspace's pendingKindTrash=4 constant and historical
// fuzz corpora keep matching without change.
func TestFuzzLegacyPendingKind_Trash(t *testing.T) {
	var m Model
	m.guard.kind = guardTrash
	if got := m.fuzzLegacyPendingKind(); got != 4 {
		t.Errorf("fuzzLegacyPendingKind() with guard.kind=guardTrash = %d, want 4", got)
	}
}

// TestFuzzLegacyPendingKind_ConflictCoexistence pins critic R1's coexistence
// window directly on the adapter: a conflict guard raised mid-close-save
// leaves guard.kind reading guardConflict (raiseConflictGuard overwrote it)
// while guard.close.active stays true — fuzzLegacyPendingKind must keep
// reporting Close=1 in that window, not fall through to None=0, exactly
// mirroring pendingDataLoss.kind's pre-A4 independence from guard.kind (see
// TestConflictDuringCloseSave_CoexistsThenAbandonsClose for the full
// end-to-end version of this scenario).
func TestFuzzLegacyPendingKind_ConflictCoexistence(t *testing.T) {
	var m Model
	m.guard.kind = guardConflict
	m.guard.close = closeIntent{active: true}
	if got := m.fuzzLegacyPendingKind(); got != 1 {
		t.Errorf("fuzzLegacyPendingKind() with guard.kind=guardConflict and guard.close.active=true = %d, want 1 (Close)", got)
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
