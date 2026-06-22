package search

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/keybind"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newTestBar creates a search bar with a real registry and resolver — the same
// setup the workspace uses in production. Without these the field cannot process
// character input (edit.insert-character is not in the empty default registry).
func newTestBar(t *testing.T) Model {
	t.Helper()
	keys := keymap.Default()
	builder := command.NewBuilder()
	var err error
	builder, err = textedit.RegisterCommands(builder)
	if err != nil {
		t.Fatalf("RegisterCommands: %v", err)
	}
	reg := builder.Build()
	bindings, err := keys.CommandBindings()
	if err != nil {
		t.Fatalf("CommandBindings: %v", err)
	}
	resolver, err := keybind.NewResolver(bindings)
	if err != nil {
		t.Fatalf("NewResolver: %v", err)
	}
	m := New(keys, styles.Default(),
		textedit.WithRegistry(reg),
		textedit.WithResolver(resolver),
	)
	m = m.SetSize(80, 1)
	return m
}

func typeText(m Model, s string) Model {
	for _, r := range s {
		m, _ = m.Update(tea.KeyPressMsg{Text: string(r)})
	}
	return m
}

func execCmd(t *testing.T, cmd tea.Cmd) tea.Msg {
	t.Helper()
	if cmd == nil {
		return nil
	}
	return cmd()
}

// TestSearchBar_TypingUpdatesQuery verifies that typing characters while the
// bar is open and focused actually inserts them into the query. This test
// requires a real registry; without it edit.insert-character silently fails.
func TestSearchBar_TypingUpdatesQuery(t *testing.T) {
	m := newTestBar(t).Open()
	m = typeText(m, "foo")
	if got := m.Query(); got != "foo" {
		t.Errorf("Query() = %q, want %q", got, "foo")
	}
}

func TestSearchBar_BackspaceRemovesChar(t *testing.T) {
	m := newTestBar(t).Open()
	m = typeText(m, "ab")
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyBackspace})
	if got := m.Query(); got != "a" {
		t.Errorf("after backspace: Query() = %q, want %q", got, "a")
	}
}

func TestSearchBar_EnterEmitsSubmitForward(t *testing.T) {
	m := newTestBar(t).Open()
	m = typeText(m, "foo")
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	msg := execCmd(t, cmd)
	sub, ok := msg.(SubmitMsg)
	if !ok {
		t.Fatalf("expected SubmitMsg, got %T", msg)
	}
	if sub.Query != "foo" || sub.Backward {
		t.Errorf("SubmitMsg = %+v, want {Query:\"foo\", Backward:false}", sub)
	}
}

func TestSearchBar_ShiftEnterEmitsSubmitBackward(t *testing.T) {
	m := newTestBar(t).Open()
	m = typeText(m, "bar")
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter, Mod: tea.ModShift})
	msg := execCmd(t, cmd)
	sub, ok := msg.(SubmitMsg)
	if !ok {
		t.Fatalf("expected SubmitMsg, got %T", msg)
	}
	if sub.Query != "bar" || !sub.Backward {
		t.Errorf("SubmitMsg = %+v, want {Query:\"bar\", Backward:true}", sub)
	}
}

func TestSearchBar_EscapeEmitsClose(t *testing.T) {
	m := newTestBar(t).Open()
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	msg := execCmd(t, cmd)
	if _, ok := msg.(CloseMsg); !ok {
		t.Errorf("expected CloseMsg, got %T", msg)
	}
}

func TestSearchBar_ClosedBarIgnoresInput(t *testing.T) {
	m := newTestBar(t) // not opened
	m = typeText(m, "abc")
	if got := m.Query(); got != "" {
		t.Errorf("closed bar: Query() = %q, want empty", got)
	}
}

func TestSearchBar_UnfocusedOpenBarIgnoresInput(t *testing.T) {
	m := newTestBar(t).Open()
	m = m.SetFocused(false)
	m = typeText(m, "abc")
	if got := m.Query(); got != "" {
		t.Errorf("unfocused bar: Query() = %q, want empty", got)
	}
}

// TestSearchBar_HistoryNavigation verifies Up/Down history cycling. History is
// stored recent-first; Up enters at index 0 (most recent), subsequent Up moves
// toward older entries; Down reverses; Down past the top restores the draft.
func TestSearchBar_HistoryNavigation(t *testing.T) {
	m := newTestBar(t).Open()
	m = m.SetHistory([]string{"second", "first"}) // recent-first

	// First Up: enter history at most-recent entry.
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyUp})
	if got := m.Query(); got != "second" {
		t.Errorf("after 1st Up: Query() = %q, want %q", got, "second")
	}

	// Second Up: move to older entry.
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyUp})
	if got := m.Query(); got != "first" {
		t.Errorf("after 2nd Up: Query() = %q, want %q", got, "first")
	}

	// Down from oldest: move back toward recent.
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if got := m.Query(); got != "second" {
		t.Errorf("after Down: Query() = %q, want %q", got, "second")
	}

	// Down past most-recent: restore draft (empty).
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if got := m.Query(); got != "" {
		t.Errorf("after Down past top: Query() = %q, want empty draft", got)
	}
}
