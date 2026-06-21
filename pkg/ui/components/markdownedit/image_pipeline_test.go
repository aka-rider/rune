package markdownedit

import (
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/display"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/image"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newImagePipelineModel builds a focused, sized markdownedit model wired with
// the real command registry + resolver, so arrow keys resolve to cursor moves.
func newImagePipelineModel(t *testing.T, caps terminal.TermCaps) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	builder := command.NewBuilder()
	builder, err := RegisterCommands(builder)
	if err != nil {
		t.Fatalf("register commands: %v", err)
	}
	reg := builder.Build()
	cmdBindings, err := keys.CommandBindings()
	if err != nil {
		t.Fatalf("command bindings: %v", err)
	}
	res, err := keybind.NewResolver(cmdBindings)
	if err != nil {
		t.Fatalf("new resolver: %v", err)
	}
	m := New(keys, st, caps, WithRegistry(reg), WithResolver(res))
	m = m.SetRect(textedit.Rect{X: 0, Y: 0, W: 40, H: 10})
	m = m.SetFocused(true)
	return m
}

// TestImageRowExpansionStableAcrossCursorMoves locks in the fix for the
// disappearing-text bug: image-row expansion must be an invariant of the
// display snapshot, re-applied on every rebuild. Before the fix, a cursor move
// rebuilt the snapshot via syncDisplay without re-expanding (revision
// unchanged), so the image collapsed on each keypress while async messages
// re-expanded it — the row count oscillated, the viewport desynced, and text
// jumped/vanished.
func TestImageRowExpansionStableAcrossCursorMoves(t *testing.T) {
	m := newImagePipelineModel(t, terminal.TermCaps{})

	const imgRows = 5
	const textLines = 4 // B, C, D, E
	// Use a non-empty alt: an empty-alt "![](a.png)" parses to a malformed
	// 1-char image span plus a leftover "[](a.png)" text span, so the line is
	// not a standalone image line and never expands. Production embeds carry a
	// label (alt text or the "![[name]]" wikilink form), so this matches reality.
	m = m.SetContent("![alt](a.png)\nB\nC\nD\nE")
	// Publish the (live) image footprint straight into the display pipeline.
	// Decode/transmit I/O is irrelevant to row expansion, so we bypass it.
	m.Model = m.Model.SetImageDims(map[string]display.ImageDims{"a.png": {Cols: 8, Rows: imgRows}})

	// Cursor starts on the image line (offset 0): the embed is revealed as
	// source and occupies a single row. 5 model lines => 5 display rows.
	if got := m.Model.Snapshot().TotalRows; got != 1+textLines {
		t.Fatalf("cursor on image line: TotalRows=%d, want %d (collapsed source)", got, 1+textLines)
	}

	down := tea.KeyPressMsg{Code: tea.KeyDown}
	up := tea.KeyPressMsg{Code: tea.KeyUp}
	wantExpanded := imgRows + textLines

	// Move off the image line: it renders as an image and expands to imgRows.
	m, _ = m.Update(down)
	if got := m.Model.Snapshot().TotalRows; got != wantExpanded {
		t.Fatalf("after moving off image line: TotalRows=%d, want %d", got, wantExpanded)
	}

	// Every further cursor move keeps the snapshot stable — never collapsing
	// (the original bug) and never doubling (re-expanding an expanded snapshot).
	for i := 0; i < 3; i++ {
		m, _ = m.Update(down)
		if got := m.Model.Snapshot().TotalRows; got != wantExpanded {
			t.Fatalf("cursor-down #%d: TotalRows=%d, want stable %d", i+1, got, wantExpanded)
		}
	}

	// Returning onto the image line reveals the source (collapse); leaving it
	// re-expands cleanly.
	for i := 0; i < textLines; i++ {
		m, _ = m.Update(up)
	}
	if got := m.Model.Snapshot().TotalRows; got != 1+textLines {
		t.Fatalf("back on image line: TotalRows=%d, want %d (collapsed)", got, 1+textLines)
	}
	m, _ = m.Update(down)
	if got := m.Model.Snapshot().TotalRows; got != wantExpanded {
		t.Fatalf("re-expansion after collapse: TotalRows=%d, want %d", got, wantExpanded)
	}
}

// TestPendingImageReservesNoRows locks in the secondary fix for the
// black/empty-area symptom: an image that has not yet transmitted its pixels to
// the terminal must reserve only a single row, so the editor never emits blank
// placeholder cells pointing at an image the terminal has no data for yet.
//
// PendingDecode images have Height()==1 (rows field is 0 until decode completes),
// so the Height()>1 gate in currentImageDims correctly excludes them even after
// the IsLive() guard was removed in Fix A.
func TestPendingImageReservesNoRows(t *testing.T) {
	caps := terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true}
	m := newImagePipelineModel(t, caps)

	// A freshly-created image is PendingDecode — not Live — so it contributes
	// no expanded footprint.
	img := image.New("a.png", "/abs/a.png", 1, 0, m.termCaps, m.cellSize, 40, 10)
	if img.IsLive() {
		t.Fatalf("freshly-created image should not be Live")
	}
	m.images["a.png"] = img

	if dims := m.currentImageDims(); dims != nil {
		t.Fatalf("PendingDecode image must reserve no expanded rows, got %v", dims)
	}
}

// TestVisibleRowsForCountsOnscreenRows guards the animation-gating fix: an
// animated image only ticks while on-screen, and visibleRowsFor is what feeds
// image.SetVisibleRows in updateImages. With the image expanded and fully in
// view it must report all its rows; with the cursor on the embed (revealed to
// source) it collapses to a single non-image line and must report 0, pausing
// the animation while the user edits the source.
func TestVisibleRowsForCountsOnscreenRows(t *testing.T) {
	const imgRows = 5
	m := newImagePipelineModel(t, terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true})
	m = m.SetContent("![alt](a.png)\nB\nC\nD\nE")
	m.Model = m.Model.SetImageDims(map[string]display.ImageDims{"a.png": {Cols: 8, Rows: imgRows}})

	// Cursor on the image line → revealed source, no expanded image rows.
	if got := m.visibleRowsFor("a.png"); got != 0 {
		t.Fatalf("cursor on embed: visibleRowsFor=%d, want 0 (revealed source)", got)
	}

	// Move off the embed → it expands to imgRows, all within the 10-row viewport.
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if got := m.visibleRowsFor("a.png"); got != imgRows {
		t.Fatalf("image expanded in view: visibleRowsFor=%d, want %d", got, imgRows)
	}
	// An untracked path reports nothing.
	if got := m.visibleRowsFor("missing.png"); got != 0 {
		t.Fatalf("unknown image: visibleRowsFor=%d, want 0", got)
	}
}

// TestDiscoverImagesOnLoad guards the file-load discovery path: calling
// DiscoverImages on a freshly-SetContent editor (no buffer edit ever made)
// must track standalone image embeds and return a decode Cmd. This is the
// primary regression from the first fix — discovery was gated on
// afterContentChange which required a buffer revision bump, so a file opened
// without an edit never discovered its images.
func TestDiscoverImagesOnLoad(t *testing.T) {
	// Create a temp directory with a stub image file so resolveEmbed succeeds.
	tmpDir := t.TempDir()
	imgFile := filepath.Join(tmpDir, "photo.webp")
	if err := os.WriteFile(imgFile, []byte("RIFF"), 0o644); err != nil {
		t.Fatalf("create stub image: %v", err)
	}

	keys := keymap.Default()
	st := styles.Default()
	// Build an unfocused model — the state at file-load time (focus is on the
	// file tree, not the editor). All image spans are Rendered in this state,
	// so StandaloneImagePath returns them. Real graphics caps are required:
	// discoverNewImages is gated on imageCapable(), and a file is only ever
	// loaded into an editor running in a real (graphics-capable) terminal.
	m := New(keys, st, terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true})
	m = m.SetRect(textedit.Rect{X: 0, Y: 0, W: 40, H: 10})
	m = m.SetDir(tmpDir)
	m = m.SetContent("![[photo.webp]]")

	// No buffer edit has been made — images must still be discovered.
	m2, cmd := m.DiscoverImages()
	if _, tracked := m2.images["photo.webp"]; !tracked {
		t.Fatal("DiscoverImages: image not tracked in m.images after SetContent; discovery requires a buffer edit (regression)")
	}
	if cmd == nil {
		t.Fatal("DiscoverImages: expected non-nil decode Cmd, got nil")
	}
}
