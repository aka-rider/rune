//go:build fuzzing

package driver

import (
	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/event"
)

// bindingTable maps a key index (from fuzz input) to a tea.KeyPressMsg.
// Modular indexing: KeyIndex % len(bindingTable) is always in range.
var bindingTable = []tea.KeyPressMsg{
	// Navigation
	{Code: tea.KeyUp},
	{Code: tea.KeyDown},
	{Code: tea.KeyLeft},
	{Code: tea.KeyRight},
	{Code: tea.KeyHome},
	{Code: tea.KeyEnd},
	{Code: tea.KeyPgUp},
	{Code: tea.KeyPgDown},
	// Editing
	{Code: tea.KeyEnter},
	{Code: tea.KeyBackspace},
	{Code: tea.KeyDelete},
	{Code: tea.KeyTab},
	{Code: tea.KeyTab, Mod: tea.ModShift},
	// Workspace globals
	{Code: 'o', Mod: tea.ModCtrl},  // ZenMode
	{Code: 'w', Mod: tea.ModCtrl},  // CloseFile
	{Code: 'x', Mod: tea.ModCtrl},  // FocusExplorer
	{Code: 'e', Mod: tea.ModCtrl},  // FocusEditor
	{Code: 'r', Mod: tea.ModCtrl},  // FocusChat
	{Code: 'n', Mod: tea.ModCtrl},  // CreateNewFile
	{Code: 'p', Mod: tea.ModCtrl},  // PinTab
	{Code: 'u', Mod: tea.ModCtrl},  // HalfPageUp
	{Code: 'd', Mod: tea.ModCtrl},  // HalfPageDown
	{Code: 'a', Mod: tea.ModCtrl},  // SelectAll
	{Code: 'c', Mod: tea.ModCtrl},  // ConfirmExitC
	// Selection extensions
	{Code: tea.KeyUp, Mod: tea.ModShift},
	{Code: tea.KeyDown, Mod: tea.ModShift},
	{Code: tea.KeyLeft, Mod: tea.ModShift},
	{Code: tea.KeyRight, Mod: tea.ModShift},
	// Word movement
	{Code: tea.KeyLeft, Mod: tea.ModAlt},
	{Code: tea.KeyRight, Mod: tea.ModAlt},
	// Line movement
	{Code: tea.KeyUp, Mod: tea.ModAlt},
	{Code: tea.KeyDown, Mod: tea.ModAlt},
	// Escape
	{Code: tea.KeyEscape},
	// Characters (common printables for text insertion)
	{Code: 'a'}, {Code: 'b'}, {Code: 'c'}, {Code: 'd'}, {Code: 'e'},
	{Code: 'f'}, {Code: 'g'}, {Code: 'h'}, {Code: 'i'}, {Code: 'j'},
	{Code: 'k'}, {Code: 'l'}, {Code: 'm'}, {Code: 'n'}, {Code: 'o'},
	{Code: 'p'}, {Code: 'q'}, {Code: 'r'}, {Code: 's'}, {Code: 't'},
	{Code: 'u'}, {Code: 'v'}, {Code: 'w'}, {Code: 'x'}, {Code: 'y'},
	{Code: 'z'}, {Code: ' '}, {Code: '.'}, {Code: ','}, {Code: '\n'},
	// File ops
	{Code: 's', Mod: tea.ModSuper},    // super+s = SaveFile
	{Code: 'z', Mod: tea.ModSuper},    // super+z = Undo
	{Code: 'y', Mod: tea.ModCtrl},     // ctrl+y = Redo
}

// eventToMsg converts a fuzz Event to a tea.Msg.
// Returns nil if the event has no corresponding message (should be skipped).
func eventToMsg(ev event.Event) tea.Msg {
	switch ev.Kind {
	case event.KindKey:
		idx := int(ev.KeyIndex) % len(bindingTable)
		return bindingTable[idx]
	case event.KindText:
		if len(ev.Text) == 0 {
			return nil
		}
		// Insert the first rune as a printable key
		runes := []rune(ev.Text)
		return tea.KeyPressMsg{Code: runes[0]}
	case event.KindResize:
		return tea.WindowSizeMsg{Width: int(ev.Width), Height: int(ev.Height)}
	case event.KindPaste:
		if len(ev.Text) == 0 {
			return nil
		}
		return tea.PasteMsg{Content: ev.Text}
	case event.KindFocus:
		// Map pane index to one of 3 focus key presses.
		focusKeys := [3]tea.KeyPressMsg{
			{Code: 'x', Mod: tea.ModCtrl}, // FocusExplorer
			{Code: 'e', Mod: tea.ModCtrl}, // FocusEditor
			{Code: 'r', Mod: tea.ModCtrl}, // FocusChat
		}
		return focusKeys[int(ev.PaneIndex)%3]
	default:
		return nil
	}
}
