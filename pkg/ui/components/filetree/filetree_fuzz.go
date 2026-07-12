package filetree

// FuzzCursor delegates to the always-available Cursor() for fuzz harness use.
func (m Model) FuzzCursor() int { return m.Cursor() }

// FuzzLen delegates to the always-available Len() for fuzz harness use.
func (m Model) FuzzLen() int { return m.Len() }
