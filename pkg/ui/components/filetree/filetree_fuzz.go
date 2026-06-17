//go:build fuzzing

package filetree

// FuzzCursor returns the current cursor index in the file list.
func (m Model) FuzzCursor() int { return m.cursor }

// FuzzLen returns the number of entries in the file list.
func (m Model) FuzzLen() int { return len(m.entries) }
