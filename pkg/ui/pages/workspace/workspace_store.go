//go:build !fuzzing

package workspace

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
)

func openStoreCmd() tea.Cmd {
	return func() tea.Msg {
		store, warn, err := docstate.Open()
		if err != nil {
			return ErrMsg{Err: fmt.Errorf("open storage: %w", err)}
		}
		return StoreReadyMsg{Store: store, Warning: warn}
	}
}
