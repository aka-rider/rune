package textedit

import (
	tea "charm.land/bubbletea/v2"
)

// FindOverlay manages the find-in-file UI state.
type FindOverlay struct {
	visible    bool
	replace    bool
	searchText string
	replaceText string
	cursorPos  int // cursor position within the search text
}

func (o FindOverlay) open(withReplace bool) FindOverlay {
	return FindOverlay{
		visible:   true,
		replace:   withReplace,
		cursorPos: 0,
	}
}

func (o FindOverlay) close() FindOverlay {
	return FindOverlay{visible: false}
}

// Visible reports whether the overlay is currently shown.
func (o FindOverlay) Visible() bool { return o.visible }

// consumeKey handles key presses when the overlay is visible.
// Returns the updated overlay and whether the key was consumed.
func (o FindOverlay) consumeKey(msg tea.KeyPressMsg) (FindOverlay, bool) {
	if !o.visible {
		return o, false
	}

	switch {
	case msg.Code == tea.KeyEsc:
		return o.close(), true
	case msg.Code == tea.KeyEnter:
		// Trigger find (no-op in textedit; real find is in markdownedit)
		return o, true
	case msg.Code == tea.KeyBackspace:
		if o.cursorPos > 0 {
			o.cursorPos--
			o.searchText = o.searchText[:o.cursorPos]
			return o, true
		}
		return o, true
	case msg.Code == tea.KeyTab:
		if o.replace {
			o.cursorPos = len(o.searchText)
			return o, true
		}
		return o, true
	default:
		if msg.Code >= tea.KeySpace && msg.Code <= '~' {
			o.searchText += string(msg.Code)
			o.cursorPos = len(o.searchText)
			return o, true
		}
	}

	return o, false
}
