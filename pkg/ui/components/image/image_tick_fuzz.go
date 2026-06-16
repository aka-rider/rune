//go:build fuzzing

package image

import (
	"time"

	tea "charm.land/bubbletea/v2"
)

func (m Model) scheduleFrame(gen, next int, d time.Duration) tea.Cmd { return nil }
