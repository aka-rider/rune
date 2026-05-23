package keymap

import "charm.land/bubbles/v2/key"

type Bindings struct {
	Up, Down       key.Binding
	GotoTop        key.Binding
	GotoBottom     key.Binding
	Select         key.Binding
	CycleLeftFocus key.Binding
	ZenMode        key.Binding
	CloseFile      key.Binding
	PageUp         key.Binding
	PageDown       key.Binding
	HalfPageUp     key.Binding
	HalfPageDown   key.Binding
	TabSwitch      key.Binding
	ConfirmExitC   key.Binding
	ConfirmExitD   key.Binding
	PinTab         key.Binding
	FocusExplorer  key.Binding
	FocusEditor    key.Binding
	HelpExpand     key.Binding
}

func Default() Bindings {
	return Bindings{
		Up:             key.NewBinding(key.WithKeys("up", "k"), key.WithHelp("↑/k", "up")),
		Down:           key.NewBinding(key.WithKeys("down", "j"), key.WithHelp("↓/j", "down")),
		GotoTop:        key.NewBinding(key.WithKeys("home", "g"), key.WithHelp("g", "top")),
		GotoBottom:     key.NewBinding(key.WithKeys("end", "G"), key.WithHelp("G", "bottom")),
		Select:         key.NewBinding(key.WithKeys("enter"), key.WithHelp("↵", "open")),
		CycleLeftFocus: key.NewBinding(key.WithKeys("tab"), key.WithHelp("tab", "cycle left")),
		ZenMode:        key.NewBinding(key.WithKeys("esc"), key.WithHelp("esc", "zen")),
		CloseFile:      key.NewBinding(key.WithKeys("ctrl+w"), key.WithHelp("^w", "close")),
		PageUp:         key.NewBinding(key.WithKeys("pgup", "b"), key.WithHelp("pgup", "page up")),
		PageDown:       key.NewBinding(key.WithKeys("pgdown", "f"), key.WithHelp("pgdn", "page down")),
		HalfPageUp:     key.NewBinding(key.WithKeys("u"), key.WithHelp("u", "½ up")),
		HalfPageDown:   key.NewBinding(key.WithKeys("d"), key.WithHelp("d", "½ down")),
		TabSwitch:      key.NewBinding(key.WithKeys("ctrl+1", "ctrl+2", "ctrl+3", "ctrl+4", "ctrl+5", "ctrl+6", "ctrl+7", "ctrl+8", "ctrl+9"), key.WithHelp("^1-9", "switch tab")),
		ConfirmExitC:   key.NewBinding(key.WithKeys("ctrl+c"), key.WithHelp("^c", "exit")),
		ConfirmExitD:   key.NewBinding(key.WithKeys("ctrl+d"), key.WithHelp("^d", "exit")),
		PinTab:         key.NewBinding(key.WithKeys("ctrl+p"), key.WithHelp("^p", "pin tab")),
		FocusExplorer:  key.NewBinding(key.WithKeys("ctrl+x"), key.WithHelp("^x", "explorer")),
		FocusEditor:    key.NewBinding(key.WithKeys("ctrl+e"), key.WithHelp("^e", "editor")),
		HelpExpand:     key.NewBinding(key.WithKeys("backspace"), key.WithHelp("^?", "help")),
	}
}

// HelpEntry holds a key label and its description for footer display.
type HelpEntry struct {
	Key  string
	Desc string
}

// HelpText returns all help entries derived from the keymap bindings.
// The footer uses this as its sole source of truth instead of duplicating text.
func (b Bindings) HelpText() []HelpEntry {
	return []HelpEntry{
		{b.Up.Help().Key, b.Up.Help().Desc},
		{b.Down.Help().Key, b.Down.Help().Desc},
		{b.GotoTop.Help().Key, b.GotoTop.Help().Desc},
		{b.GotoBottom.Help().Key, b.GotoBottom.Help().Desc},
		{b.Select.Help().Key, b.Select.Help().Desc},
		{b.CycleLeftFocus.Help().Key, b.CycleLeftFocus.Help().Desc},
		{b.ZenMode.Help().Key, b.ZenMode.Help().Desc},
		{b.CloseFile.Help().Key, b.CloseFile.Help().Desc},
		{b.PageUp.Help().Key, b.PageUp.Help().Desc},
		{b.PageDown.Help().Key, b.PageDown.Help().Desc},
		{b.HalfPageUp.Help().Key, b.HalfPageUp.Help().Desc},
		{b.HalfPageDown.Help().Key, b.HalfPageDown.Help().Desc},
		{b.TabSwitch.Help().Key, b.TabSwitch.Help().Desc},
		{b.ConfirmExitC.Help().Key, b.ConfirmExitC.Help().Desc},
		{b.ConfirmExitD.Help().Key, b.ConfirmExitD.Help().Desc},
		{b.PinTab.Help().Key, b.PinTab.Help().Desc},
		{b.FocusExplorer.Help().Key, b.FocusExplorer.Help().Desc},
		{b.FocusEditor.Help().Key, b.FocusEditor.Help().Desc},
		{b.HelpExpand.Help().Key, b.HelpExpand.Help().Desc},
	}
}
