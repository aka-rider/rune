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
	{Code: 'o', Mod: tea.ModCtrl}, // ZenMode
	{Code: 'w', Mod: tea.ModCtrl}, // CloseFile
	{Code: 'x', Mod: tea.ModCtrl}, // FocusExplorer
	{Code: 'e', Mod: tea.ModCtrl}, // FocusEditor
	{Code: 'r', Mod: tea.ModCtrl}, // FocusChat
	{Code: 'n', Mod: tea.ModCtrl}, // CreateNewFile
	{Code: 'p', Mod: tea.ModCtrl}, // PinTab
	{Code: 'u', Mod: tea.ModCtrl}, // HalfPageUp
	{Code: 'd', Mod: tea.ModCtrl}, // HalfPageDown
	{Code: 'a', Mod: tea.ModCtrl}, // SelectAll
	{Code: 'c', Mod: tea.ModCtrl}, // ConfirmExitC
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
	// Characters (common printables for text insertion).
	// Text is set on every unmodified printable: footer.Update's guard
	// resolution matches msg.Text == string(opt.Key) && msg.Mod == 0 (empty
	// Text means the s/d/m/y guard branches never fire under the fuzzer).
	// keybind.ChordFromKeyMsg zeros Text before computing its match key, and
	// mergemode.HandleKey matches msg.Code only, so this is regression-neutral
	// for command/chord dispatch — only guard-option matching depends on it.
	{Code: 'a', Text: "a"}, {Code: 'b', Text: "b"}, {Code: 'c', Text: "c"}, {Code: 'd', Text: "d"}, {Code: 'e', Text: "e"},
	{Code: 'f', Text: "f"}, {Code: 'g', Text: "g"}, {Code: 'h', Text: "h"}, {Code: 'i', Text: "i"}, {Code: 'j', Text: "j"},
	{Code: 'k', Text: "k"}, {Code: 'l', Text: "l"}, {Code: 'm', Text: "m"}, {Code: 'n', Text: "n"}, {Code: 'o', Text: "o"},
	{Code: 'p', Text: "p"}, {Code: 'q', Text: "q"}, {Code: 'r', Text: "r"}, {Code: 's', Text: "s"}, {Code: 't', Text: "t"},
	{Code: 'u', Text: "u"}, {Code: 'v', Text: "v"}, {Code: 'w', Text: "w"}, {Code: 'x', Text: "x"}, {Code: 'y', Text: "y"},
	{Code: 'z', Text: "z"}, {Code: ' ', Text: " "}, {Code: '.', Text: "."}, {Code: ',', Text: ","}, {Code: '\n', Text: "\n"},
	// Markdown metacharacters — needed to exercise Rendered/Revealed span paths
	{Code: '*', Text: "*"}, {Code: '#', Text: "#"}, {Code: '|', Text: "|"}, {Code: '[', Text: "["},
	{Code: ']', Text: "]"}, {Code: '!', Text: "!"}, {Code: '_', Text: "_"}, {Code: '-', Text: "-"},
	{Code: '>', Text: ">"}, {Code: '`', Text: "`"}, {Code: '(', Text: "("}, {Code: ')', Text: ")"},
	{Code: '~', Text: "~"},
	// File ops
	{Code: 's', Mod: tea.ModSuper}, // super+s = SaveFile
	{Code: 'z', Mod: tea.ModSuper}, // super+z = Undo
	{Code: 'y', Mod: tea.ModCtrl},  // ctrl+y = Redo
	// Search / find — previously missing; these open and drive the search bar
	{Code: 'f', Mod: tea.ModCtrl},                 // ^F = InFileSearch open
	{Code: 'f', Mod: tea.ModShift | tea.ModSuper}, // ⇧⌘F = InFileSearch open (alt)
	{Code: 'g', Mod: tea.ModSuper},                // ⌘G = FindNext
	{Code: 'g', Mod: tea.ModShift | tea.ModSuper}, // ⇧⌘G = FindPrev
	{Code: 'f', Mod: tea.ModAlt | tea.ModSuper},   // ⌥⌘F = FindReplaceOpen
	// Explorer action
	{Code: tea.KeyBackspace, Mod: tea.ModSuper}, // super+backspace = TrashFile (84)

	// WP3 appends (append-only — indices below are load-bearing for
	// workflow.go's cluster scripts; never renumber/reorder existing entries).
	{Code: 'c', Mod: tea.ModSuper},                       // 85: ⌘C = CopyToClipboard
	{Code: 'x', Mod: tea.ModSuper},                       // 86: ⌘X = CutToClipboard
	{Code: 'v', Mod: tea.ModSuper},                       // 87: ⌘V = PasteFromClipboard (request phase; response via KindClipboard)
	{Code: tea.KeyUp, Mod: tea.ModAlt | tea.ModSuper},    // 88: ⌥⌘↑ = AddCursorAbove
	{Code: tea.KeyDown, Mod: tea.ModAlt | tea.ModSuper},  // 89: ⌥⌘↓ = AddCursorBelow
	{Code: tea.KeyF1},                                    // 90: F1 = Help
	{Code: '1', Mod: tea.ModCtrl},                        // 91: ^1 = TabSwitch(0)
	{Code: '2', Mod: tea.ModCtrl},                        // 92: ^2 = TabSwitch(1)
	{Code: '3', Mod: tea.ModCtrl},                        // 93: ^3 = TabSwitch(2)
	{Code: '4', Mod: tea.ModCtrl},                        // 94: ^4 = TabSwitch(3)
	{Code: '5', Mod: tea.ModCtrl},                        // 95: ^5 = TabSwitch(4)
	{Code: '6', Mod: tea.ModCtrl},                        // 96: ^6 = TabSwitch(5)
	{Code: '7', Mod: tea.ModCtrl},                        // 97: ^7 = TabSwitch(6)
	{Code: '8', Mod: tea.ModCtrl},                        // 98: ^8 = TabSwitch(7)
	{Code: '9', Mod: tea.ModCtrl},                        // 99: ^9 = TabSwitch(8)
	{Code: '0', Mod: tea.ModCtrl},                        // 100: ^0 = TabSwitch(9)
	{Code: 'v', Mod: tea.ModCtrl},                        // 101: ^V = VoiceDictation (safe only under the fuzzing stub)
	{Code: tea.KeyLeft, Mod: tea.ModAlt | tea.ModShift},  // 102: ⌥⇧← = ShiftWordLeft
	{Code: tea.KeyRight, Mod: tea.ModAlt | tea.ModShift}, // 103: ⌥⇧→ = ShiftWordRight
	{Code: tea.KeyHome, Mod: tea.ModShift},               // 104: ⇧home = ShiftGotoTop
	{Code: tea.KeyEnd, Mod: tea.ModShift},                // 105: ⇧end = ShiftGotoBottom
	{Code: tea.KeyPgUp, Mod: tea.ModShift},               // 106: ⇧pgup = ShiftPageUp
	{Code: tea.KeyPgDown, Mod: tea.ModShift},             // 107: ⇧pgdown = ShiftPageDown
	// Multi-byte printables — drive edit.insert-character with real
	// multi-byte input, a path distinct from tea.PasteMsg's whole-string
	// insert.
	{Code: '你', Text: "你"}, // 108: CJK
	{Code: '🙂', Text: "🙂"}, // 109: emoji (surrogate-pair-width rune)
	{Code: 'é', Text: "é"}, // 110: precomposed Latin accent
}

// eventToMsg converts a fuzz Event to a tea.Msg.
// Returns nil if the event has no corresponding message (should be skipped).
// KindWatch and KindExternalWrite are handled separately in RunHuman; here
// they fall through to nil so Run (the low-level fuzzer) ignores them.
func eventToMsg(ev event.Event) tea.Msg {
	switch ev.Kind {
	case event.KindKey:
		idx := int(ev.KeyIndex) % len(bindingTable)
		return bindingTable[idx]
	case event.KindText:
		if len(ev.Text) == 0 {
			return nil
		}
		// Paste the full text so multi-char / multi-line states are reachable.
		return tea.PasteMsg{Content: ev.Text}
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
		// KindWatch and KindExternalWrite are handled by RunHuman, not here.
		return nil
	}
}
