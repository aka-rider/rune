package markdownedit

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

// newLinkModel builds a focused, sized markdownedit model wired with the real
// command registry + resolver (so off-link Enter resolves to edit.newline), with
// an empty doc path (relative links resolve against the workspace root only).
func newLinkModel(t *testing.T, content string) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	builder, err := RegisterCommands(command.NewBuilder())
	if err != nil {
		t.Fatalf("register commands: %v", err)
	}
	cmdBindings, err := keys.CommandBindings()
	if err != nil {
		t.Fatalf("command bindings: %v", err)
	}
	res, err := keybind.NewResolver(cmdBindings)
	if err != nil {
		t.Fatalf("new resolver: %v", err)
	}
	m := New(keys, st, terminal.TermCaps{}, WithRegistry(builder.Build()), WithResolver(res))
	m = m.SetRect(textedit.Rect{X: 0, Y: 0, W: 80, H: 24})
	m = m.SetContent(content)
	m = m.SetFocused(true)
	return m
}

func mustWrite(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

// collectLinkActivated runs cmd (unwrapping a tea.Batch) and reports the first
// LinkActivatedMsg it produces, if any.
func collectLinkActivated(cmd tea.Cmd) (LinkActivatedMsg, bool) {
	if cmd == nil {
		return LinkActivatedMsg{}, false
	}
	switch msg := cmd().(type) {
	case LinkActivatedMsg:
		return msg, true
	case tea.BatchMsg:
		for _, c := range msg {
			if la, ok := collectLinkActivated(c); ok {
				return la, true
			}
		}
	}
	return LinkActivatedMsg{}, false
}

func TestIsExternalURL(t *testing.T) {
	external := []string{"http://x.com", "https://x.com", "HTTPS://X.COM", "mailto:a@b.com", "  https://x.com  "}
	internal := []string{"", "note.md", "./rel.md", "/abs/path.md", "data:image/png;base64,AAAA", "ftp://x"}
	for _, s := range external {
		if !isExternalURL(s) {
			t.Errorf("isExternalURL(%q) = false, want true", s)
		}
	}
	for _, s := range internal {
		if isExternalURL(s) {
			t.Errorf("isExternalURL(%q) = true, want false", s)
		}
	}
}

// TestLinkAt covers external + non-navigable spans (no filesystem). Internal
// resolution (which existence-checks) is in TestResolveRefAgainstFileDirThenRoot.
func TestLinkAt(t *testing.T) {
	tests := []struct {
		name     string
		content  string
		offset   int
		wantOK   bool
		wantKind LinkKind
		wantDest string
	}{
		{"external http", "[a](https://example.com)", 1, true, LinkExternal, "https://example.com"},
		{"external mailto", "[a](mailto:x@y.com)", 1, true, LinkExternal, "mailto:x@y.com"},
		{"image not navigable", "![a](img.png)", 5, false, 0, ""},
		{"data uri not navigable", "[a](data:xyz)", 1, false, 0, ""},
		{"plain text no link", "hello world", 3, false, 0, ""},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			m := newLinkModel(t, tt.content)
			la, ok := m.linkAt(tt.offset)
			if ok != tt.wantOK {
				t.Fatalf("linkAt ok=%v want %v (la=%+v)", ok, tt.wantOK, la)
			}
			if !ok {
				return
			}
			if la.Kind != tt.wantKind || la.Dest != tt.wantDest {
				t.Errorf("got Kind=%d Dest=%q; want Kind=%d Dest=%q", la.Kind, la.Dest, tt.wantKind, tt.wantDest)
			}
		})
	}
}

// TestResolveRefAgainstFileDirThenRoot is the core Round-3 guarantee: a relative
// link resolves against the open document's OWN folder first (golden docPath),
// then the workspace root — never the process CWD. A target that exists nowhere
// is LinkMissing (still a hit, so the follow reports it and the hint shows it).
func TestResolveRefAgainstFileDirThenRoot(t *testing.T) {
	dir := t.TempDir()  // the document's folder
	root := t.TempDir() // the workspace root (base #2)
	if err := os.MkdirAll(filepath.Join(dir, "pages"), 0o755); err != nil {
		t.Fatal(err)
	}
	mustWrite(t, filepath.Join(dir, "pages", "y.md"), "y")
	mustWrite(t, filepath.Join(root, "q.md"), "q")

	content := "[a](pages/y.md)\n[b](q.md)\n[c](missing.md)"
	m := newLinkModel(t, content)
	m = m.SetDocPath(filepath.Join(dir, "note.md")).SetRoot(root)

	// [a](pages/y.md): exists under the document's own folder → LinkInternal.
	if la, ok := m.linkAt(1); !ok || la.Kind != LinkInternal || la.Dest != filepath.Join(dir, "pages", "y.md") {
		t.Errorf("pages/y.md: got %+v ok=%v; want LinkInternal %s", la, ok, filepath.Join(dir, "pages", "y.md"))
	}
	// [b](q.md): not in the doc folder → falls back to the workspace root.
	l2 := len("[a](pages/y.md)\n") + 1
	if la, ok := m.linkAt(l2); !ok || la.Kind != LinkInternal || la.Dest != filepath.Join(root, "q.md") {
		t.Errorf("q.md (root fallback): got %+v ok=%v; want %s", la, ok, filepath.Join(root, "q.md"))
	}
	// [c](missing.md): nowhere → LinkMissing, Raw preserved.
	l3 := len("[a](pages/y.md)\n[b](q.md)\n") + 1
	if la, ok := m.linkAt(l3); !ok || la.Kind != LinkMissing || la.Raw != "missing.md" {
		t.Errorf("missing.md: got %+v ok=%v; want LinkMissing raw=missing.md", la, ok)
	}
}

// TestResolveRefUsesInjectedFS proves §1.4.9: link resolution consults the
// injected vfs.FS, not real disk. The target exists ONLY in vfs.Mem (there is no
// /vault/b.md on disk), so resolving it to LinkInternal is impossible with the old
// os.Stat — it can only succeed through the injected filesystem.
func TestResolveRefUsesInjectedFS(t *testing.T) {
	mem := vfs.NewMem()
	if err := mem.WriteFile("/vault/b.md", []byte("# B"), 0o644); err != nil {
		t.Fatal(err)
	}

	content := "[to b](b.md)\n[dead](missing.md)"
	m := newLinkModel(t, content)
	m = m.SetFS(mem).SetDocPath("/vault/a.md").SetRoot("/vault")

	// [to b](b.md): present only in the in-memory FS → LinkInternal via fsys.Stat.
	if la, ok := m.linkAt(1); !ok || la.Kind != LinkInternal || la.Dest != "/vault/b.md" {
		t.Errorf("b.md via vfs.Mem: got %+v ok=%v; want LinkInternal /vault/b.md", la, ok)
	}
	// [dead](missing.md): absent from the FS → LinkMissing.
	off := len("[to b](b.md)\n") + 1
	if la, ok := m.linkAt(off); !ok || la.Kind != LinkMissing {
		t.Errorf("missing.md: got %+v ok=%v; want LinkMissing", la, ok)
	}
}

// TestLinkAtCursorReturnsRawTarget: the footer hint shows the link as written,
// even when the target does not exist (LinkMissing).
// TestUntitledImagePasteUsesWorkspaceRoot locks in review-#2: pasting an image
// into an untitled doc (no docPath) must NOT silently no-op — it falls back to the
// workspace root and produces a save Cmd.
func TestUntitledImagePasteUsesWorkspaceRoot(t *testing.T) {
	dir := t.TempDir()
	m := New(keymap.Default(), styles.Default(), terminal.TermCaps{}).SetRoot(dir)
	_, cmd := m.handleImagePaste([]byte{0x89, 'P', 'N', 'G'}, "image/png", time.Now())
	if cmd == nil {
		t.Fatal("untitled image paste produced no Cmd (silent no-op regression)")
	}
}

func TestLinkAtCursorReturnsRawTarget(t *testing.T) {
	m := newLinkModel(t, "[x](pages/y.md)") // caret at offset 0 → on the link
	target, ok := m.LinkAtCursor()
	if !ok || target != "pages/y.md" {
		t.Errorf("LinkAtCursor = %q, %v; want pages/y.md, true", target, ok)
	}
}

func TestEnterFollowsExternalLink(t *testing.T) {
	m := newLinkModel(t, "[a](https://example.com)") // caret at offset 0 after SetContent
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	got, ok := collectLinkActivated(cmd)
	if !ok {
		t.Fatal("Enter on a link did not emit LinkActivatedMsg")
	}
	if got.Kind != LinkExternal || got.Dest != "https://example.com" {
		t.Errorf("got %+v, want LinkExternal https://example.com", got)
	}
}

func TestCtrlEnterIsAliasForEnter(t *testing.T) {
	m := newLinkModel(t, "[a](https://example.com)")
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter, Mod: tea.ModCtrl})
	if got, ok := collectLinkActivated(cmd); !ok || got.Kind != LinkExternal || got.Dest != "https://example.com" {
		t.Errorf("Ctrl+Enter should follow like Enter; got %+v ok=%v", got, ok)
	}
}

func TestEnterOffLinkDoesNotFollow(t *testing.T) {
	m := newLinkModel(t, "hello") // no link under the caret
	before := m.Content()
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if _, ok := collectLinkActivated(cmd); ok {
		t.Fatal("Enter off a link must not follow")
	}
	if m2, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter}); m2.Content() == before {
		t.Error("Enter off a link should insert a newline (content unchanged)")
	}
}

func TestEnterWithSelectionDoesNotFollow(t *testing.T) {
	m := newLinkModel(t, "[a](https://example.com)")
	// A single caret WITH a selection must not be treated as "on a link".
	m = m.SetCursors([]cursor.Cursor{{Position: 2, Anchor: 0, ID: 1}})
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if _, ok := collectLinkActivated(cmd); ok {
		t.Fatal("Enter with an active selection must not follow")
	}
}

func TestDoubleClickFollowsLink(t *testing.T) {
	m := newLinkModel(t, "[a](https://example.com)")
	click := tea.MouseClickMsg{X: 0, Y: 0, Button: tea.MouseLeft}

	// First click: positions the caret / reveals — no follow.
	m, cmd := m.Update(click)
	if _, ok := collectLinkActivated(cmd); ok {
		t.Fatal("single click must not follow")
	}
	// Second click at the same spot within the threshold: follow.
	_, cmd = m.Update(click)
	got, ok := collectLinkActivated(cmd)
	if !ok {
		t.Fatal("double click on a link did not emit LinkActivatedMsg")
	}
	if got.Kind != LinkExternal || got.Dest != "https://example.com" {
		t.Errorf("got %+v, want LinkExternal https://example.com", got)
	}
}

// TestClickOnConcealedLinkLandsInSpan locks in the §1.5 byte/rune seam: with the
// caret on line 2, line 1's link renders concealed ("click"), hiding the
// delimiters and URL. Clicking its visible text must map the display column back
// to a BYTE offset inside the link span — not a rune-shifted one — so the caret
// lands in the link and reveals it.
func TestClickOnConcealedLinkLandsInSpan(t *testing.T) {
	m := newLinkModel(t, "[click](http://e.com)\nsecond line")
	m = m.SetCursors([]cursor.Cursor{{Position: 25, Anchor: 25, ID: 1}}) // within "second line"
	m, _ = m.Update(tea.MouseClickMsg{X: 3, Y: 0, Button: tea.MouseLeft}) // over "click" on line 1
	if _, ok := m.linkAt(m.CursorOffset()); !ok {
		t.Fatalf("click on concealed link text landed at offset %d, outside the link span", m.CursorOffset())
	}
}

func TestDoubleClickOnPlainTextDoesNotFollow(t *testing.T) {
	m := newLinkModel(t, "hello world")
	click := tea.MouseClickMsg{X: 2, Y: 0, Button: tea.MouseLeft}
	m, _ = m.Update(click)
	_, cmd := m.Update(click)
	if _, ok := collectLinkActivated(cmd); ok {
		t.Error("double click on plain text must not follow (it selects a word)")
	}
}
