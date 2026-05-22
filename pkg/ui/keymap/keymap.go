package keymap

import "charm.land/bubbles/v2/key"

type Bindings struct {
	Up, Down        key.Binding
	GotoTop         key.Binding
	GotoBottom      key.Binding
	Select          key.Binding
	FocusLeft       key.Binding
	FocusCenter     key.Binding
	CycleLeftFocus  key.Binding
	ZenMode         key.Binding
	CloseFile       key.Binding
	PageUp          key.Binding
	PageDown        key.Binding
	HalfPageUp      key.Binding
	HalfPageDown    key.Binding
	Quit            key.Binding
}

func Default() Bindings {
	return Bindings{
		Up:             key.NewBinding(key.WithKeys("up", "k"),       key.WithHelp("↑/k", "up")),
		Down:           key.NewBinding(key.WithKeys("down", "j"),     key.WithHelp("↓/j", "down")),
		GotoTop:        key.NewBinding(key.WithKeys("home", "g"),     key.WithHelp("g", "top")),
		GotoBottom:     key.NewBinding(key.WithKeys("end", "G"),      key.WithHelp("G", "bottom")),
		Select:         key.NewBinding(key.WithKeys("enter"),         key.WithHelp("↵", "open")),
		FocusLeft:      key.NewBinding(key.WithKeys("ctrl+1"),        key.WithHelp("^1", "left pane")),
		FocusCenter:    key.NewBinding(key.WithKeys("ctrl+2"),        key.WithHelp("^2", "editor")),
		CycleLeftFocus: key.NewBinding(key.WithKeys("tab"),           key.WithHelp("tab", "cycle left")),
		ZenMode:        key.NewBinding(key.WithKeys("esc"),           key.WithHelp("esc", "zen")),
		CloseFile:      key.NewBinding(key.WithKeys("ctrl+w"),        key.WithHelp("^w", "close")),
		PageUp:         key.NewBinding(key.WithKeys("pgup", "b"),     key.WithHelp("pgup", "page up")),
		PageDown:       key.NewBinding(key.WithKeys("pgdown", "f"),   key.WithHelp("pgdn", "page down")),
		HalfPageUp:     key.NewBinding(key.WithKeys("u"),             key.WithHelp("u", "½ up")),
		HalfPageDown:   key.NewBinding(key.WithKeys("d"),             key.WithHelp("d", "½ down")),
		Quit:           key.NewBinding(key.WithKeys("q", "ctrl+c"),   key.WithHelp("q", "quit")),
	}
}
