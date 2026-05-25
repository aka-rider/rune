package editor

import tea "charm.land/bubbletea/v2"

// FindOverlay is the internal find/replace overlay for the editor component.
type FindOverlay struct {
	visible       bool
	replaceMode   bool
	query         string
	replacement   string
	matches       []int // unused in MVP
	currentIdx    int   // unused in MVP
	caseSensitive bool  // unused in MVP
	useRegex      bool  // unused in MVP
}

func (f FindOverlay) Visible() bool { return f.visible }

func (f FindOverlay) open(replaceMode bool) FindOverlay {
	f.visible = true
	f.replaceMode = replaceMode
	return f
}

func (f FindOverlay) close() FindOverlay {
	f.visible = false
	f.query = ""
	f.replacement = ""
	return f
}

// consumeKey handles all key events when the overlay is visible.
// Returns true if the key was consumed (always true when visible).
func (f FindOverlay) consumeKey(msg tea.KeyPressMsg) (FindOverlay, bool) {
	if !f.visible {
		return f, false
	}

	// Escape closes overlay
	if msg.Code == tea.KeyEscape && msg.Mod == 0 {
		f = f.close()
		return f, true
	}

	// All other keys are consumed with no effect in MVP
	return f, true
}
