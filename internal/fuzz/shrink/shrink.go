//go:build fuzzing

package shrink

import (
	"rune/internal/fuzz/driver"
	"rune/internal/fuzz/event"
	"rune/pkg/docstate"
	"rune/pkg/ui/pages/workspace"
)

// Shrinker drives the minimization of a failing event sequence.
type Shrinker struct {
	NewModel  func() workspace.Model
	NewStore  func() (*docstate.Store, error)
	Width     int
	Height    int
	TargetID  string
}

// Shrink returns the minimal event sequence that still produces a violation
// with TargetID. Uses a greedy 1-minimal approach: remove events one by one,
// keep the removal if it still reproduces. O(n²) but correct and simple.
func (s Shrinker) Shrink(events []event.Event) []event.Event {
	events = append([]event.Event(nil), events...)
	for {
		reduced := false
		for i := 0; i < len(events); i++ {
			candidate := make([]event.Event, 0, len(events)-1)
			candidate = append(candidate, events[:i]...)
			candidate = append(candidate, events[i+1:]...)
			if s.tryReproduces(candidate) {
				events = candidate
				reduced = true
				break
			}
		}
		if !reduced {
			return events
		}
	}
}

func (s Shrinker) tryReproduces(evs []event.Event) bool {
	store, err := s.NewStore()
	if err != nil {
		return false
	}
	model := s.NewModel()
	violation, _, _ := driver.Run(model, evs, store, s.Width, s.Height)
	_ = store.Close() // fire-and-forget: best-effort cleanup of in-memory store
	return violation != nil && violation.InvariantID == s.TargetID
}
