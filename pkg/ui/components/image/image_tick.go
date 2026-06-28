//go:build !fuzzing

package image

import (
	"time"

	tea "charm.land/bubbletea/v2"
)

func (m Model) scheduleFrame(gen, next int, d time.Duration) tea.Cmd {
	p, g, n, dur := m.path, gen, next, d
	return tea.Tick(dur, func(time.Time) tea.Msg {
		return UpdateMsg{Path: p, inner: frameTickMsg{path: p, gen: g, next: n}}
	})
}
