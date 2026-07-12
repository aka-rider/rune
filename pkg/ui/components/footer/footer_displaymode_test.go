package footer

import (
	"testing"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// displayModeCase names one displayMode and how to populate the Model state
// that makes it active, independent of every other mode's state.
type displayModeCase struct {
	name  string
	mode  displayMode
	apply func(Model) Model
}

// displayModeCases lists every non-default mode in PRIORITY ORDER, highest
// first — this order mirrors the doc comment on the displayMode type and IS
// the table under test.
func displayModeCases() []displayModeCase {
	return []displayModeCase{
		{"error", modeError, func(m Model) Model {
			m.errorMsg = "boom"
			return m
		}},
		{"dictating", modeDictating, func(m Model) Model {
			m.dictating = true
			return m
		}},
		{"guard", modeGuard, func(m Model) Model {
			return m.SetGuard(GuardDirty, []GuardOption{{Key: 's', Response: DataLossSave}})
		}},
		{"chordPending", modeChordPending, func(m Model) Model {
			m.pendingKey = "c"
			return m
		}},
		{"mergeHint", modeMergeHint, func(m Model) Model {
			return m.SetMergeMode(true, 1)
		}},
		{"diskChanged", modeDiskChanged, func(m Model) Model {
			return m.SetDiskChanged(true)
		}},
		{"degraded", modeDegraded, func(m Model) Model {
			return m.SetDegraded(true)
		}},
		{"status", modeStatus, func(m Model) Model {
			m.statusMsg = "hi"
			return m
		}},
		{"linkHint", modeLinkHint, func(m Model) Model {
			m.linkHint = "foo.md"
			return m
		}},
	}
}

// TestDisplayMode_PriorityOrder mechanically proves the single ordered
// priority table (§1.4.4): for every ordered pair of (higher-priority,
// lower-priority) states, populating BOTH simultaneously must still yield
// the higher-priority mode. This is the guarantee that a transient
// status/linkHint (or any lower-priority state) can never mask a guard —
// and, more generally, that no state above it in the table can be masked by
// one below it.
func TestDisplayMode_PriorityOrder(t *testing.T) {
	cases := displayModeCases()
	base := New(keymap.Default(), styles.Default()).SetSize(80, 1)

	for hi := 0; hi < len(cases); hi++ {
		for lo := hi + 1; lo < len(cases); lo++ {
			higher, lower := cases[hi], cases[lo]
			t.Run(higher.name+"_over_"+lower.name, func(t *testing.T) {
				m := base
				m = higher.apply(m)
				m = lower.apply(m)
				if got := m.displayMode(); got != higher.mode {
					t.Fatalf("with both %s and %s state populated, displayMode() = %v, want %v (%s)",
						higher.name, lower.name, got, higher.mode, higher.name)
				}
			})
		}
	}
}

// TestDisplayMode_DefaultWhenNothingSet: with no state populated, the mode
// falls through to modeDefault (the always-visible global-shortcut hints).
func TestDisplayMode_DefaultWhenNothingSet(t *testing.T) {
	m := New(keymap.Default(), styles.Default()).SetSize(80, 1)
	if got := m.displayMode(); got != modeDefault {
		t.Fatalf("displayMode() with nothing set = %v, want modeDefault", got)
	}
}
