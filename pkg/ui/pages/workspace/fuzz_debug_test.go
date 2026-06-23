//go:build fuzzing

package workspace_test

import (
	"fmt"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"rune/pkg/command"
	"rune/pkg/docstate"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

func FuzzDebugTabChurnManual(f *testing.F) {
	mem := vfs.NewMem()
	_ = mem.WriteFile("/fuzz/a.md", []byte("# File A\n\nInitial content of A.\n"), 0o644)
	_ = mem.WriteFile("/fuzz/b.md", []byte("# File B\n\nInitial content of B.\n"), 0o644)
	_ = mem.WriteFile("/fuzz/notes/c.md", []byte("# Notes\n\n- item one\n- item two\n"), 0o644)

	store, err := docstate.OpenInMemory(time.Now)
	if err != nil {
		f.Fatal(err)
	}
	defer store.Close()
	store.UseFS(mem)

	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)
	caps := terminal.TermCaps{}

	m := workspace.New(keys, st, reg, res, caps, "/fuzz", []string{"/fuzz/a.md"}).WithFS(mem)

	// Bootstrap manually
	m = bootstrapFull(m, store, 80, 24)

	// Print state after bootstrap
	snap := m.FuzzInspect()
	fmt.Printf("After bootstrap: tabs=%d activeIdx=%d active=%v docID=%d path=%q focus=%d\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath, snap.FocusPane)

	// Event 0: evNew (Ctrl+N)
	m = stepKey(m, 'n', tea.ModCtrl)
	snap = m.FuzzInspect()
	fmt.Printf("After evNew#1: tabs=%d activeIdx=%d active=%v docID=%d path=%q focus=%d\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath, snap.FocusPane)

	// Event 1: evNew
	m = stepKey(m, 'n', tea.ModCtrl)
	snap = m.FuzzInspect()
	fmt.Printf("After evNew#2: tabs=%d activeIdx=%d active=%v docID=%d path=%q focus=%d\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath, snap.FocusPane)

	// Event 2: evNew
	m = stepKey(m, 'n', tea.ModCtrl)
	snap = m.FuzzInspect()
	fmt.Printf("After evNew#3: tabs=%d activeIdx=%d active=%v docID=%d path=%q focus=%d\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath, snap.FocusPane)

	// Event 3: evPin
	m = stepKey(m, 'p', tea.ModCtrl)
	snap = m.FuzzInspect()
	fmt.Printf("After evPin: tabs=%d activeIdx=%d active=%v docID=%d path=%q focus=%d\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath, snap.FocusPane)

	// Event 4: evClose
	m = stepKey(m, 'w', tea.ModCtrl)
	snap = m.FuzzInspect()
	fmt.Printf("After evClose#1: tabs=%d activeIdx=%d active=%v docID=%d path=%q focus=%d\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath, snap.FocusPane)

	// Event 5: evClose
	m = stepKey(m, 'w', tea.ModCtrl)
	snap = m.FuzzInspect()
	fmt.Printf("After evClose#2: tabs=%d activeIdx=%d active=%v docID=%d path=%q focus=%d\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath, snap.FocusPane)
}

func stepKey(m workspace.Model, code rune, mod tea.KeyMod) workspace.Model {
	msg := tea.KeyPressMsg{Code: code, Mod: mod}
	fmt.Printf("\n=== Key: Ctrl+%c ===\n", code)
	snap := m.FuzzInspect()
	fmt.Printf("  Before: tabs=%d activeIdx=%d active=%v docID=%d path=%q focus=%d\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath, snap.FocusPane)
	m, cmd := m.Update(msg)
	snap = m.FuzzInspect()
	fmt.Printf("  After Update: tabs=%d activeIdx=%d active=%v docID=%d path=%q focus=%d\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath, snap.FocusPane)
	if cmd != nil {
		for {
			c := cmd()
			if c == nil {
				break
			}
			m2, cmd2 := m.Update(c)
			if cmd2 != nil {
				cmd = cmd2
				m = m2
				continue
			}
			m = m2
		}
		snap = m.FuzzInspect()
		fmt.Printf("  After cmds: tabs=%d activeIdx=%d active=%v docID=%d path=%q focus=%d\n",
			snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath, snap.FocusPane)
	}
	return m
}

func bootstrapFull(m workspace.Model, store *docstate.Store, w, h int) workspace.Model {
	m, _ = m.Update(tea.WindowSizeMsg{Width: w, Height: h})
	snap := m.FuzzInspect()
	fmt.Printf("After WSSizeMsg: tabs=%d activeIdx=%d active=%v docID=%d path=%q\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath)
	if initCmd := m.Init(); initCmd != nil {
		cmd := initCmd
		for cmd != nil {
			msg := cmd()
			if msg != nil {
				m2, cmd2 := m.Update(msg)
				if cmd2 != nil {
					cmd = cmd2
					m = m2
					continue
				}
				m = m2
			} else {
				cmd = nil
			}
		}
	}
	m, _ = m.Update(workspace.StoreReadyMsg{Store: store})
	snap = m.FuzzInspect()
	fmt.Printf("After StoreReady: tabs=%d activeIdx=%d active=%v docID=%d path=%q\n",
		snap.TabCount, snap.ActiveTabIdx, snap.TabActive, snap.DocID, snap.ActiveFilePath)
	return m
}
