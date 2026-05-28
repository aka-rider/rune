//go:build !darwin

package dictation

import (
	"context"
	"errors"

	tea "charm.land/bubbletea/v2"
)

// StartCmd is not supported on non-darwin platforms.
func StartCmd(_ context.Context, _ Config) tea.Cmd {
	return func() tea.Msg {
		return ErrorMsg{
			Err:   errors.New("voice dictation not supported on this platform"),
			Fatal: true,
		}
	}
}
