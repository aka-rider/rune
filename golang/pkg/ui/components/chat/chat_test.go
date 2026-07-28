package chat

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ai"
	"rune/pkg/command"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func newTestModel(t *testing.T) Model {
	t.Helper()
	builder := command.NewBuilder()
	reg := builder.Build()
	resolver, err := keybind.NewResolver(nil)
	if err != nil {
		t.Fatalf("NewResolver: %v", err)
	}
	// New no longer constructs the client itself (§2.5): mirror workspace.New's
	// call-site construction so this test keeps New's original behavior.
	client, clientErr := ai.NewClient()
	return New(keymap.Default(), styles.Default(), reg, resolver, terminal.TermCaps{}, client, clientErr)
}

// A failed ai.NewClient leaves a zero-value Client on the model; submit must
// refuse instead of firing a doomed HTTP request with an empty key at the
// live endpoint (and, under the fuzzing build, driving real network I/O from
// inside a fuzz worker). Regression for the submit-ignores-initErr defect.
func TestSubmit_RefusedWhenClientInitFailed(t *testing.T) {
	m := newTestModel(t)
	m.initErr = "OPENAI_API_KEY is required when using the public OpenAI endpoint"
	m = m.SetFocused(true)

	m.prompt = m.prompt.SetContent("hello")

	m, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if cmd != nil {
		t.Fatalf("submit with initErr must not produce a Cmd (would fire a real HTTP request); got %T", cmd)
	}
	if m.loading {
		t.Fatal("submit with initErr must not enter the loading state")
	}
	if len(m.messages) != 0 {
		t.Fatalf("submit with initErr must not append messages; got %d", len(m.messages))
	}
	if got := m.prompt.Content(); got != "hello" {
		t.Fatalf("refused submit must preserve the typed prompt; got %q", got)
	}
}
