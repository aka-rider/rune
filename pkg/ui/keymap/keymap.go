package keymap

import (
	"fmt"
	"reflect"
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
	FocusChat             key.Binding
	CreateNewFile         key.Binding
	Help                  key.Binding
	Backspace             key.Binding
	Delete                key.Binding
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
	WordLeft              key.Binding
	WordRight             key.Binding
	ShiftWordLeft         key.Binding
	ShiftWordRight        key.Binding
	MoveLineUp            key.Binding
	MoveLineDown          key.Binding
	SelectAll             key.Binding
	CopyToClipboard       key.Binding
	CutToClipboard        key.Binding
	PasteFromClipboard    key.Binding
	VoiceDictation        key.Binding
	Undo                  key.Binding
	Redo                  key.Binding
}

func Default() Bindings {
	return Bindings{
		Up:                 key.NewBinding(key.WithKeys("up"), key.WithHelp("↑", "up")),
		Down:               key.NewBinding(key.WithKeys("down"), key.WithHelp("↓", "down")),
		Left:               key.NewBinding(key.WithKeys("left"), key.WithHelp("←", "left")),
		Right:              key.NewBinding(key.WithKeys("right"), key.WithHelp("→", "right")),
		GotoTop:            key.NewBinding(key.WithKeys("home"), key.WithHelp("home", "top")),
		GotoBottom:         key.NewBinding(key.WithKeys("end"), key.WithHelp("end", "bottom")),
		PrimaryAction:      key.NewBinding(key.WithKeys("enter"), key.WithHelp("↵", "open/newline")),
		Cancel:             key.NewBinding(key.WithKeys("esc"), key.WithHelp("esc", "cancel")),
		ZenMode:            key.NewBinding(key.WithKeys("ctrl+o"), key.WithHelp("^o", "zen")),
		CloseFile:          key.NewBinding(key.WithKeys("ctrl+w"), key.WithHelp("^w", "close")),
		PageUp:             key.NewBinding(key.WithKeys("pgup"), key.WithHelp("pgup", "page up")),
		PageDown:           key.NewBinding(key.WithKeys("pgdown"), key.WithHelp("pgdn", "page down")),
		HalfPageUp:         key.NewBinding(key.WithKeys("ctrl+u"), key.WithHelp("^u", "½ up")),
		HalfPageDown:       key.NewBinding(key.WithKeys("ctrl+d"), key.WithHelp("^d", "½ down")),
		TabSwitch:          key.NewBinding(key.WithKeys("ctrl+1", "ctrl+2", "ctrl+3", "ctrl+4", "ctrl+5", "ctrl+6", "ctrl+7", "ctrl+8", "ctrl+9"), key.WithHelp("^1-9", "switch tab")),
		ConfirmExitC:       key.NewBinding(key.WithKeys("ctrl+c"), key.WithHelp("^c", "exit")),
		ConfirmExitD:       key.NewBinding(key.WithKeys("ctrl+alt+d"), key.WithHelp("⌥^d", "exit")),
		PinTab:             key.NewBinding(key.WithKeys("ctrl+p"), key.WithHelp("^p", "pin tab")),
		FocusExplorer:      key.NewBinding(key.WithKeys("ctrl+x"), key.WithHelp("^x", "explorer")),
		FocusEditor:        key.NewBinding(key.WithKeys("ctrl+e"), key.WithHelp("^e", "editor")),
		FocusChat:          key.NewBinding(key.WithKeys("ctrl+r"), key.WithHelp("^r", "chat")),
		CreateNewFile:      key.NewBinding(key.WithKeys("ctrl+n"), key.WithHelp("^n", "new file")),
		Help:               key.NewBinding(key.WithKeys("f1"), key.WithHelp("F1", "help")),
		Backspace:          key.NewBinding(key.WithKeys("backspace"), key.WithHelp("⌫", "delete")),
		Delete:             key.NewBinding(key.WithKeys("delete"), key.WithHelp("⌦", "delete right")),
		Indent:             key.NewBinding(key.WithKeys("tab"), key.WithHelp("tab", "indent")),
		Outdent:            key.NewBinding(key.WithKeys("shift+tab"), key.WithHelp("⇧tab", "outdent")),
		SaveFile:           key.NewBinding(key.WithKeys("super+s"), key.WithHelp("⌘s", "save")),
		AddCursorAbove:     key.NewBinding(key.WithKeys("alt+super+up"), key.WithHelp("⌥⌘↑", "cursor above")),
		AddCursorBelow:     key.NewBinding(key.WithKeys("alt+super+down"), key.WithHelp("⌥⌘↓", "cursor below")),
		FindOpen:           key.NewBinding(key.WithKeys("shift+super+f", "ctrl+f"), key.WithHelp("⇧⌘f / ^F", "find")),
		FindReplaceOpen:    key.NewBinding(key.WithKeys("alt+super+f", "ctrl+alt+f"), key.WithHelp("⌥⌘f", "find & replace")),
		FindNext:           key.NewBinding(key.WithKeys("super+g"), key.WithHelp("⌘g", "find next")),
		FindPrev:           key.NewBinding(key.WithKeys("shift+super+g"), key.WithHelp("⇧⌘g", "find prev")),
		ShiftUp:            key.NewBinding(key.WithKeys("shift+up"), key.WithHelp("⇧↑", "shift+up")),
		ShiftDown:          key.NewBinding(key.WithKeys("shift+down"), key.WithHelp("⇧↓", "shift+down")),
		ShiftLeft:          key.NewBinding(key.WithKeys("shift+left"), key.WithHelp("⇧←", "shift+left")),
		ShiftRight:         key.NewBinding(key.WithKeys("shift+right"), key.WithHelp("⇧→", "shift+right")),
		ShiftGotoTop:       key.NewBinding(key.WithKeys("shift+home"), key.WithHelp("⇧⌘", "shift+top")),
		ShiftGotoBottom:    key.NewBinding(key.WithKeys("shift+end"), key.WithHelp("⇧⇥", "shift+bottom")),
		ShiftPageUp:        key.NewBinding(key.WithKeys("shift+pgup"), key.WithHelp("⇧⇞", "shift+page up")),
		ShiftPageDown:      key.NewBinding(key.WithKeys("shift+pgdown"), key.WithHelp("⇧⇟", "shift+page down")),
		WordLeft:           key.NewBinding(key.WithKeys("alt+left", "alt+b"), key.WithHelp("⌥←", "word left")),
		WordRight:          key.NewBinding(key.WithKeys("alt+right", "alt+f"), key.WithHelp("⌥→", "word right")),
		ShiftWordLeft:      key.NewBinding(key.WithKeys("alt+shift+left"), key.WithHelp("⇧⌥←", "select word left")),
		ShiftWordRight:     key.NewBinding(key.WithKeys("alt+shift+right"), key.WithHelp("⇧⌥→", "select word right")),
		MoveLineUp:         key.NewBinding(key.WithKeys("alt+up"), key.WithHelp("⌥↑", "move line up")),
		MoveLineDown:       key.NewBinding(key.WithKeys("alt+down"), key.WithHelp("⌥↓", "move line down")),
		SelectAll:          key.NewBinding(key.WithKeys("super+a", "shift+super+a", "ctrl+a"), key.WithHelp("⌘a", "select all")),
		CopyToClipboard:    key.NewBinding(key.WithKeys("super+c", "ctrl+shift+c"), key.WithHelp("⌘c", "copy")),
		CutToClipboard:     key.NewBinding(key.WithKeys("super+x"), key.WithHelp("⌘x", "cut")),
		PasteFromClipboard: key.NewBinding(key.WithKeys("super+v"), key.WithHelp("⌘v", "paste")),
		VoiceDictation:     key.NewBinding(key.WithKeys("ctrl+v"), key.WithHelp("^v", "dictate")),
		Undo:               key.NewBinding(key.WithKeys("super+z", "ctrl+z"), key.WithHelp("⌘z", "undo")),
		Redo:               key.NewBinding(key.WithKeys("shift+super+z", "ctrl+y"), key.WithHelp("⇧⌘z", "redo")),
	}
}

type HelpEntry struct {
	Key  string
	Desc string
}

// eachBinding visits every key.Binding field on Bindings in declaration
// order. It is the single enumeration point for the keymap: AllPhysicalKeys,
// AllHelp, and the collision validator all build on it, so a newly added
// binding is covered automatically with no parallel list to keep in sync.
func (b Bindings) eachBinding(fn func(key.Binding)) {
	v := reflect.ValueOf(b)
	for i := 0; i < v.NumField(); i++ {
		if kb, ok := v.Field(i).Interface().(key.Binding); ok {
			fn(kb)
		}
	}
}

// AllHelp returns a key+description entry for every binding, in
// struct-declaration order. This is the source for the in-app help
// document — reflecting over the keymap guarantees the list can never
// drift from the actual bindings.
func (b Bindings) AllHelp() []HelpEntry {
	var entries []HelpEntry
	b.eachBinding(func(kb key.Binding) {
		h := kb.Help()
		if h.Key == "" {
			return
		}
		entries = append(entries, HelpEntry{Key: h.Key, Desc: h.Desc})
	})
	return entries
}

func (b Bindings) AllPhysicalKeys() []string {
	var keys []string
	b.eachBinding(func(kb key.Binding) {
		keys = append(keys, kb.Keys()...)
	})
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
		case "cmd", "super":
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
	add(b.Delete, "edit.delete-right", "editorFocused && !readOnly")
	add(b.Indent, "edit.indent", "editorFocused && !readOnly")
	add(b.Outdent, "edit.outdent", "editorFocused && !readOnly")
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
	add(b.WordLeft, "cursor.word-left", "editorFocused")
	add(b.WordRight, "cursor.word-right", "editorFocused")
	add(b.ShiftWordLeft, "select.word-left", "editorFocused")
	add(b.ShiftWordRight, "select.word-right", "editorFocused")
	add(b.MoveLineUp, "edit.move-line-up", "editorFocused && !readOnly")
	add(b.MoveLineDown, "edit.move-line-down", "editorFocused && !readOnly")
	add(b.HalfPageUp, "cursor.page-up", "editorFocused")
	add(b.HalfPageDown, "cursor.page-down", "editorFocused")
	add(b.SelectAll, "select.all", "editorFocused")
	add(b.CopyToClipboard, "clipboard.copy", "editorFocused")
	add(b.CutToClipboard, "clipboard.cut", "editorFocused && !readOnly")
	add(b.PasteFromClipboard, "clipboard.paste", "editorFocused && !readOnly")
	return mappings, parseErr
}
