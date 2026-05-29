package filetree

import tea "charm.land/bubbletea/v2"

const mouseScrollLines = 3

func (m Model) handleMouseClick(msg tea.MouseClickMsg) (Model, tea.Cmd) {
	if !m.focused {
		return m, nil
	}
	if msg.Button != tea.MouseLeft {
		return m, nil
	}

	relY := msg.Y - m.offsetY
	if relY <= 0 {
		return m, nil // title row or above
	}

	maxVisible := m.height - 1
	if maxVisible <= 0 {
		return m, nil
	}
	start := 0
	if m.cursor >= maxVisible {
		start = m.cursor - maxVisible + 1
	}

	idx := start + (relY - 1) // relY=1 → first visible entry
	if idx < 0 || idx >= len(m.entries) {
		return m, nil
	}

	if m.cursor == idx {
		e := m.entries[m.cursor]
		if e.IsDir {
			return m, func() tea.Msg { return DirSelectedMsg{Path: e.Path} }
		}
		return m, func() tea.Msg { return FileSelectedMsg{Path: e.Path} }
	}

	m.cursor = idx
	return m, nil
}

func (m Model) handleMouseWheel(msg tea.MouseWheelMsg) (Model, tea.Cmd) {
	if !m.focused {
		return m, nil
	}
	switch msg.Button {
	case tea.MouseWheelUp:
		for i := 0; i < mouseScrollLines; i++ {
			if m.cursor > 0 {
				m.cursor--
			}
		}
	case tea.MouseWheelDown:
		for i := 0; i < mouseScrollLines; i++ {
			if m.cursor < len(m.entries)-1 {
				m.cursor++
			}
		}
	}
	return m, nil
}
