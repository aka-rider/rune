package keymap

import (
	"fmt"
	"strings"

	"rune/pkg/editor/keybind"

	"charm.land/bubbles/v2/key"
)

type Bindings struct {
	Up, Down, Left, Right key.Binding
	GotoTop               key.Binding
	GotoBottom            key.Binding
	PrimaryAction         key.Binding
	Cancel                key.Binding
	ZenMode               key.Binding
	CloseFile             key.Binding
	PageUp                key.Binding
	PageDown              key.Binding
	HalfPageUp            key.Binding
	HalfPageDown          key.Binding
	TabSwitch             key.Binding
	ConfirmExitC          key.Binding
	ConfirmExitD          key.Binding
	PinTab                key.Binding
	FocusExplorer         key.Binding
	FocusEditor           key.Binding
	HelpExpand            key.Binding
	Backspace             key.Binding
	Indent                key.Binding
	Outdent               key.Binding
	SaveFile              key.Binding
	AddCursorAbove        key.Binding
	AddCursorBelow        key.Binding
	FindOpen              key.Binding
	FindReplaceOpen       key.Binding
	FindNext              key.Binding
	FindPrev              key.Binding
	ShiftUp               key.Binding
	ShiftDown             key.Binding
	ShiftLeft             key.Binding
	ShiftRight            key.Binding
	ShiftGotoTop          key.Binding
	ShiftGotoBottom       key.Binding
	ShiftPageUp           key.Binding
	ShiftPageDown         key.Binding
	SelectAll             key.Binding
}

func Default() Bindings {
	return Bindings{
		Up:              key.NewBinding(key.WithKeys("up"), key.WithHelp("↑", "up")),
		Down:            key.NewBinding(key.WithKeys("down"), key.WithHelp("↓", "down")),
		Left:            key.NewBinding(key.WithKeys("left"), key.WithHelp("←", "left")),
		Right:           key.NewBinding(key.WithKeys("right"), key.WithHelp("→", "right")),
		GotoTop:         key.NewBinding(key.WithKeys("home"), key.WithHelp("home", "top")),
		GotoBottom:      key.NewBinding(key.WithKeys("end"), key.WithHelp("end", "bottom")),
		PrimaryAction:   key.NewBinding(key.WithKeys("enter"), key.WithHelp("↵", "open/newline")),
		Cancel:          key.NewBinding(key.WithKeys("esc"), key.WithHelp("esc", "cancel")),
		ZenMode:         key.NewBinding(key.WithKeys("ctrl+o"), key.WithHelp("^o", "zen")),
		CloseFile:       key.NewBinding(key.WithKeys("ctrl+w"), key.WithHelp("^w", "close")),
		PageUp:          key.NewBinding(key.WithKeys("pgup"), key.WithHelp("pgup", "page up")),
		PageDown:        key.NewBinding(key.WithKeys("pgdown"), key.WithHelp("pgdn", "page down")),
		HalfPageUp:      key.NewBinding(key.WithKeys("ctrl+u"), key.WithHelp("^u", "½ up")),
		HalfPageDown:    key.NewBinding(key.WithKeys("ctrl+d"), key.WithHelp("^d", "½ down")),
		TabSwitch:       key.NewBinding(key.WithKeys("ctrl+1", "ctrl+2", "ctrl+3", "ctrl+4", "ctrl+5", "ctrl+6", "ctrl+7", "ctrl+8", "ctrl+9"), key.WithHelp("^1-9", "switch tab")),
		ConfirmExitC:    key.NewBinding(key.WithKeys("ctrl+c"), key.WithHelp("^c", "exit")),
		ConfirmExitD:    key.NewBinding(key.WithKeys("alt+ctrl+d"), key.WithHelp("⌥^d", "exit")),
		PinTab:          key.NewBinding(key.WithKeys("ctrl+p"), key.WithHelp("^p", "pin tab")),
		FocusExplorer:   key.NewBinding(key.WithKeys("ctrl+x"), key.WithHelp("^x", "explorer")),
		FocusEditor:     key.NewBinding(key.WithKeys("ctrl+e"), key.WithHelp("^e", "editor")),
		HelpExpand:      key.NewBinding(key.WithKeys("?"), key.WithHelp("?", "help")),
		Backspace:       key.NewBinding(key.WithKeys("backspace"), key.WithHelp("⌫", "delete")),
		Indent:          key.NewBinding(key.WithKeys("tab"), key.WithHelp("tab", "indent")),
		Outdent:         key.NewBinding(key.WithKeys("shift+tab"), key.WithHelp("⇧tab", "outdent")),
		SaveFile:        key.NewBinding(key.WithKeys("cmd+s"), key.WithHelp("⌘s", "save")),
		AddCursorAbove:  key.NewBinding(key.WithKeys("alt+cmd+up"), key.WithHelp("⌥⌘↑", "cursor above")),
		AddCursorBelow:  key.NewBinding(key.WithKeys("alt+cmd+down"), key.WithHelp("⌥⌘↓", "cursor below")),
		FindOpen:        key.NewBinding(key.WithKeys("cmd+f"), key.WithHelp("⌘f", "find")),
		FindReplaceOpen: key.NewBinding(key.WithKeys("cmd+h"), key.WithHelp("⌘h", "find & replace")),
		FindNext:        key.NewBinding(key.WithKeys("cmd+g"), key.WithHelp("⌘g", "find next")),
		FindPrev:        key.NewBinding(key.WithKeys("shift+cmd+g"), key.WithHelp("⇧⌘g", "find prev")),
		ShiftUp:         key.NewBinding(key.WithKeys("shift+up"), key.WithHelp("⇧↑", "shift+up")),
		ShiftDown:       key.NewBinding(key.WithKeys("shift+down"), key.WithHelp("⇧↓", "shift+down")),
		ShiftLeft:       key.NewBinding(key.WithKeys("shift+left"), key.WithHelp("⇧←", "shift+left")),
		ShiftRight:      key.NewBinding(key.WithKeys("shift+right"), key.WithHelp("⇧→", "shift+right")),
		ShiftGotoTop:    key.NewBinding(key.WithKeys("shift+home"), key.WithHelp("⇧⌘", "shift+top")),
		ShiftGotoBottom: key.NewBinding(key.WithKeys("shift+end"), key.WithHelp("⇧⇥", "shift+bottom")),
		ShiftPageUp:     key.NewBinding(key.WithKeys("shift+pgup"), key.WithHelp("⇧⇞", "shift+page up")),
		ShiftPageDown:   key.NewBinding(key.WithKeys("shift+pgdown"), key.WithHelp("⇧⇟", "shift+page down")),
		SelectAll:       key.NewBinding(key.WithKeys("cmd+a"), key.WithHelp("⌘a", "select all")),
	}
}

type HelpEntry struct {
	Key  string
	Desc string
}

func (b Bindings) HelpText() []HelpEntry {
	return []HelpEntry{
		{b.Up.Help().Key, b.Up.Help().Desc},
		{b.Down.Help().Key, b.Down.Help().Desc},
		{b.PrimaryAction.Help().Key, b.PrimaryAction.Help().Desc},
		{b.Cancel.Help().Key, b.Cancel.Help().Desc},
		{b.ZenMode.Help().Key, b.ZenMode.Help().Desc},
		{b.CloseFile.Help().Key, b.CloseFile.Help().Desc},
		{b.TabSwitch.Help().Key, b.TabSwitch.Help().Desc},
		{b.ConfirmExitC.Help().Key, b.ConfirmExitC.Help().Desc},
		{b.FocusExplorer.Help().Key, b.FocusExplorer.Help().Desc},
		{b.FocusEditor.Help().Key, b.FocusEditor.Help().Desc},
		{b.HelpExpand.Help().Key, b.HelpExpand.Help().Desc},
		{b.SaveFile.Help().Key, b.SaveFile.Help().Desc},
	}
}

func (b Bindings) AllPhysicalKeys() []string {
	var keys []string
	add := func(binding key.Binding) {
		keys = append(keys, binding.Keys()...)
	}
	add(b.Up)
	add(b.Down)
	add(b.Left)
	add(b.Right)
	add(b.GotoTop)
	add(b.GotoBottom)
	add(b.PrimaryAction)
	add(b.Cancel)
	add(b.ZenMode)
	add(b.CloseFile)
	add(b.PageUp)
	add(b.PageDown)
	add(b.HalfPageUp)
	add(b.HalfPageDown)
	add(b.TabSwitch)
	add(b.ConfirmExitC)
	add(b.ConfirmExitD)
	add(b.PinTab)
	add(b.FocusExplorer)
	add(b.FocusEditor)
	add(b.HelpExpand)
	add(b.Backspace)
	add(b.Indent)
	add(b.Outdent)
	add(b.SaveFile)
	add(b.AddCursorAbove)
	add(b.AddCursorBelow)
	add(b.FindOpen)
	add(b.FindReplaceOpen)
	add(b.FindNext)
	add(b.FindPrev)
	add(b.ShiftUp)
	add(b.ShiftDown)
	add(b.ShiftLeft)
	add(b.ShiftRight)
	add(b.ShiftGotoTop)
	add(b.ShiftGotoBottom)
	add(b.ShiftPageUp)
	add(b.ShiftPageDown)
	add(b.SelectAll)
	return keys
}

func (b Bindings) ValidateNoPhysicalKeyCollisions() error {
	keys := b.AllPhysicalKeys()
	seen := make(map[string]bool)
	for _, k := range keys {
		if seen[k] {
			return fmt.Errorf("duplicate physical key binding found: %q", k)
		}
		seen[k] = true
	}
	return nil
}

func parseChord(s string) []keybind.Chord {
	parts := strings.Split(s, "+")
	chord := keybind.Chord{}
	for _, p := range parts {
		switch p {
		case "ctrl":
			chord.Ctrl = true
		case "shift":
			chord.Shift = true
		case "alt":
			chord.Alt = true
		case "cmd":
			chord.Cmd = true
		default:
			chord.Key = p
		}
	}
	return []keybind.Chord{chord}
}

func (b Bindings) CommandBindings() ([]keybind.Binding, error) {
	var mappings []keybind.Binding
	var parseErr error

	add := func(binding key.Binding, command string, when string) {
		for _, k := range binding.Keys() {
			if k == "enter" || k == "esc" {
				continue
			}
			chords := parseChord(k)
			mappings = append(mappings, keybind.Binding{
				Chords:  chords,
				Command: command,
				When:    when,
			})
		}
	}

	add(b.Up, "cursor.line-up", "editorFocused")
	add(b.ShiftUp, "select.line-up", "editorFocused")
	add(b.Down, "cursor.line-down", "editorFocused")
	add(b.ShiftDown, "select.line-down", "editorFocused")
	add(b.Left, "cursor.character-left", "editorFocused")
	add(b.ShiftLeft, "select.character-left", "editorFocused")
	add(b.Right, "cursor.character-right", "editorFocused")
	add(b.ShiftRight, "select.character-right", "editorFocused")
	add(b.GotoTop, "cursor.line-start", "editorFocused")
	add(b.ShiftGotoTop, "select.line-start", "editorFocused")
	add(b.GotoBottom, "cursor.line-end", "editorFocused")
	add(b.ShiftGotoBottom, "select.line-end", "editorFocused")
	add(b.Backspace, "edit.delete-left", "editorFocused && !readOnly")
	add(b.Indent, "edit.indent", "editorFocused && !readOnly")
	add(b.Outdent, "edit.outdent", "editorFocused && !readOnly")
	add(b.SaveFile, "file.save", "editorFocused")
	add(b.AddCursorAbove, "multicursor.add-above", "editorFocused")
	add(b.AddCursorBelow, "multicursor.add-below", "editorFocused")
	add(b.FindOpen, "find.open", "editorFocused")
	add(b.FindReplaceOpen, "find.replace-open", "editorFocused")
	add(b.FindNext, "find.next", "editorFocused")
	add(b.FindPrev, "find.previous", "editorFocused")
	add(b.PageUp, "cursor.page-up", "editorFocused")
	add(b.ShiftPageUp, "select.page-up", "editorFocused")
	add(b.PageDown, "cursor.page-down", "editorFocused")
	add(b.ShiftPageDown, "select.page-down", "editorFocused")
	add(b.HalfPageUp, "cursor.page-up", "editorFocused")
	add(b.HalfPageDown, "cursor.page-down", "editorFocused")
	add(b.SelectAll, "select.all", "editorFocused")

	return mappings, parseErr
}
