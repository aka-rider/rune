package help

import (
	"strings"
	"testing"

	"rune/pkg/ui/keymap"
)

func TestDocumentContainsProseAndGeneratedKeys(t *testing.T) {
	doc := Document(keymap.Default())

	// Authored prose from the embedded template.
	for _, want := range []string{"Rune Help", "Voice input", "Obsidian vaults", "Keyboard shortcuts"} {
		if !strings.Contains(doc, want) {
			t.Errorf("help document missing prose %q", want)
		}
	}

	// The splice must have happened.
	if strings.Contains(doc, keybindingsPlaceholder) {
		t.Error("keybindings placeholder was not replaced")
	}

	// Rows generated from the live keymap (key glyph + description).
	for _, want := range []string{"| ⌘s | save |", "| ^x | explorer |", "| F1 | help |"} {
		if !strings.Contains(doc, want) {
			t.Errorf("help document missing generated row %q", want)
		}
	}
}
