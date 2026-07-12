package image

import (
	"time"

	tea "charm.land/bubbletea/v2"
)

// frameDelayDisabled collapses scheduleFrame's real delay to zero when set —
// see DisableFrameDelayForTesting (image_testing.go). Must still return a
// real Cmd (never nil): ArmTick sets m.ticking before consulting this
// return value, and the only reset lives in the frameTickMsg handler, which
// a nil Cmd never delivers — a nil Cmd would leave m.ticking permanently
// stuck true.
var frameDelayDisabled bool

func (m Model) scheduleFrame(gen, next int, d time.Duration) tea.Cmd {
	p, g, n, dur := m.path, gen, next, d
	if frameDelayDisabled {
		dur = 0
	}
	return tea.Tick(dur, func(time.Time) tea.Msg {
		return UpdateMsg{Path: p, inner: frameTickMsg{path: p, gen: g, next: n}}
	})
}
