package invariant

import (
	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/snapshot"
)

// Monitor is a stateful L2 invariant automaton. Observe is called after every
// settled message with (prev, msg, next). Reset is called once per shrink
// replay to clear automaton state so replays are deterministic.
type Monitor interface {
	Observe(prev snapshot.Snapshot, msg tea.Msg, next snapshot.Snapshot) []Violation
	Reset()
}
