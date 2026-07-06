//go:build fuzzing

package image

import (
	"time"

	tea "charm.land/bubbletea/v2"
)

// scheduleFrame collapses the real delay to zero under the fuzzing tag —
// mirrors footer_timers_fuzz.go/workspace_timers_fuzz.go's "keep the Cmd
// real, shrink the duration" pattern. Returning nil here (as this used to)
// left ArmTick's m.ticking permanently stuck true: ArmTick sets it before
// consulting this return value, and the only reset lives in the
// frameTickMsg handler, which a nil Cmd never delivers.
func (m Model) scheduleFrame(gen, next int, d time.Duration) tea.Cmd {
	p, g, n := m.path, gen, next
	return tea.Tick(0, func(time.Time) tea.Msg {
		return UpdateMsg{Path: p, inner: frameTickMsg{path: p, gen: g, next: n}}
	})
}
