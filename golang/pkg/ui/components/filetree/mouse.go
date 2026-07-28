package filetree

import (
	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/listnav"
)

func (m Model) handleMouseClick(msg tea.MouseClickMsg) (Model, tea.Cmd) {
	if !m.focused {
		return m, nil
	}
	if msg.Button != tea.MouseLeft {
		return m, nil
	}

	maxVisible := m.height - 1
	if maxVisible <= 0 {
		return m, nil
	}

	idx, ok := listnav.ClickIndex(msg.Y, m.offsetY, 1, m.nav.Top, len(m.entries))
	if !ok {
		return m, nil
	}

	if m.nav.Cursor == idx {
		e := m.entries[m.nav.Cursor]
		if e.IsDir {
			return m, func() tea.Msg { return DirSelectedMsg{Path: e.Path} }
		}
		return m, func() tea.Msg { return FileSelectedMsg{Path: e.Path} }
	}

	m.nav.Cursor = idx
	return m.ensureVisible(), nil
}

func (m Model) handleMouseWheel(msg tea.MouseWheelMsg) (Model, tea.Cmd) {
	if !m.focused {
		return m, nil
	}
	switch msg.Button {
	case tea.MouseWheelUp:
		m.nav = m.nav.Wheel(true, len(m.entries))
	case tea.MouseWheelDown:
		m.nav = m.nav.Wheel(false, len(m.entries))
	}
	return m.ensureVisible(), nil
}
