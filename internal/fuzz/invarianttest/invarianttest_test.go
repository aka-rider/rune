package invarianttest_test

// Smoke coverage for every exported checker: each one must accept a freshly
// constructed, minimally exercised component Model without tripping — proving
// the snapshot builders map real component state (not just the workspace's
// FuzzInspect composition) and keeping the component entry points compiling
// against the component APIs they wrap. Component packages adopt these
// checkers from their own EXTERNAL test files (see the package doc's
// import-cycle constraint); until each does, this file is the executable
// proof the seam works.

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/invarianttest"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func TestCheckTextedit_FreshAndEdited(t *testing.T) {
	m := textedit.New(keymap.Default(), styles.Default())
	m = m.SetRect(textedit.Rect{W: 40, H: 10})
	invarianttest.CheckTextedit(t, m)

	m = m.SetFocused(true)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'h', Text: "h"})
	m, _ = m.Update(tea.KeyPressMsg{Code: 'i', Text: "i"})
	invarianttest.CheckTextedit(t, m)
}

func TestCheckMarkdownedit_FreshAndEdited(t *testing.T) {
	m := markdownedit.New(keymap.Default(), styles.Default(), terminal.TermCaps{})
	m = m.SetRect(textedit.Rect{W: 40, H: 10})
	invarianttest.CheckMarkdownedit(t, m)

	m = m.SetFocused(true)
	m, _ = m.Update(tea.KeyPressMsg{Code: '#', Text: "#"})
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
	invarianttest.CheckMarkdownedit(t, m)
}

func TestCheckOpenTabs_FreshAndPopulated(t *testing.T) {
	m := opentabs.New(keymap.Default(), styles.Default())
	invarianttest.CheckOpenTabs(t, m)

	// OpenFile alone leaves no tab active — activation is the CALLER's
	// contract (every workspace call site pairs OpenFile with SetActive), and
	// TAB-SET encodes exactly that pairing.
	m = m.OpenFile(1, "/docs/a.md")
	m = m.OpenFile(2, "/docs/b.md")
	m = m.SetActive(opentabs.TabHandle{DocID: 2, Path: "/docs/b.md"})
	invarianttest.CheckOpenTabs(t, m)
}

func TestCheckFooter_FreshAndStatus(t *testing.T) {
	m := footer.New(keymap.Default(), styles.Default())
	invarianttest.CheckFooter(t, m)

	m, _ = m.Update(footer.ShowStatusMsg{Text: "saved"})
	invarianttest.CheckFooter(t, m)
}
