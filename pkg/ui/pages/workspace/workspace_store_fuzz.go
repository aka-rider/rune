//go:build fuzzing

package workspace

import (
	tea "charm.land/bubbletea/v2"

	"rune/pkg/vfs"
)

func openStoreCmd(vfs.FS, string) tea.Cmd { return nil }
