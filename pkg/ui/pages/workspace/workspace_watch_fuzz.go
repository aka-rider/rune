//go:build fuzzing

package workspace

import (
	"context"

	tea "charm.land/bubbletea/v2"
)

func watchDirCmd(ctx context.Context, dir string) tea.Cmd { return nil }
