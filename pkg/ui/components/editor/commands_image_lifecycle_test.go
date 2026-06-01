package editor

import (
	"fmt"
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/cursor"
	"rune/pkg/terminal"
)

// setCursor places a single cursor at the given byte offset.
func setCursor(m Model, offset int) Model {
	m.cursors = cursor.NewCursorSet(offset)
	return m
}

// runCmd executes a tea.Cmd, flattening tea.BatchMsg into a list of messages.
func runCmd(t *testing.T, cmd tea.Cmd) []tea.Msg {
	t.Helper()
	if cmd == nil {
		return nil
	}
	msg := cmd()
	if batch, ok := msg.(tea.BatchMsg); ok {
		var out []tea.Msg
		for _, c := range batch {
			out = append(out, runCmd(t, c)...)
		}
		return out
	}
	if msg == nil {
		return nil
	}
	return []tea.Msg{msg}
}

func firstMsg[T tea.Msg](msgs []tea.Msg) (T, bool) {
	for _, m := range msgs {
		if t, ok := m.(T); ok {
			return t, true
		}
	}
	var zero T
	return zero, false
}

// writePNG writes a w x h solid PNG to path (creating parent dirs).
func writePNG(t *testing.T, path string, w, h int) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			img.Set(x, y, color.RGBA{R: 30, G: 90, B: 200, A: 255})
		}
	}
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := png.Encode(f, img); err != nil {
		t.Fatal(err)
	}
}

// docEditor builds a kitty-capable editor whose open file lives in dir, with the
// given content, cursor on line 0.
func docEditor(t *testing.T, dir, content string) Model {
	t.Helper()
	m := newTestEditor("")
	m.termCaps = kittyCaps()
	m = m.SetContent(filepath.Join(dir, "note.md"), []byte(content))
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	return m
}

func TestImageLifecycle_DecodeTransmitLive(t *testing.T) {
	dir := t.TempDir()
	writePNG(t, filepath.Join(dir, "assets", "x.png"), 100, 100)

	m := docEditor(t, dir, "intro\n![alt](assets/x.png)\noutro")

	// Discovery should produce a DecodeImageCmd for the local image.
	m, cmd := m.discoverNewImages()
	if cmd == nil {
		t.Fatal("expected a decode Cmd from discoverNewImages")
	}
	if e, ok := m.images.get("assets/x.png"); !ok || e.state != pendingDecode {
		t.Fatalf("expected pendingDecode registry entry, got %+v ok=%v", e, ok)
	}

	msgs := runCmd(t, cmd)
	decoded, ok := firstMsg[ImageDecodedMsg](msgs)
	if !ok {
		t.Fatalf("expected ImageDecodedMsg, got %+v", msgs)
	}
	if decoded.Cols <= 0 || decoded.Rows <= 1 {
		t.Fatalf("expected multi-row image footprint, got %dx%d", decoded.Cols, decoded.Rows)
	}

	rowsBefore := m.snapshot.TotalRows

	// Feed the decoded msg back: rows reserve, transmit dispatched.
	m, cmd = m.Update(decoded)
	if m.snapshot.TotalRows <= rowsBefore {
		t.Errorf("expected snapshot to reserve more rows after decode: %d -> %d", rowsBefore, m.snapshot.TotalRows)
	}
	if e, _ := m.images.get("assets/x.png"); e.state != pendingTransmit {
		t.Errorf("expected pendingTransmit, got %v", e.state)
	}
	tmsgs := runCmd(t, cmd)
	// We can't write to a real tty in tests, so the transmit may error; either
	// outcome exercises the Cmd. We only require the Cmd existed.
	if cmd == nil {
		t.Error("expected a TransmitImageCmd after decode")
	}
	_ = tmsgs

	// Mark transmitted -> live.
	m, _ = m.Update(ImageTransmittedMsg{Path: "assets/x.png"})
	if e, _ := m.images.get("assets/x.png"); e.state != live {
		t.Errorf("expected live after ImageTransmittedMsg, got %v", e.state)
	}
}

// TestImageLifecycle_MissingFileNotDiscovered verifies that with resolveEmbed's
// exist-check, a markdown image whose file does not exist on disk resolves to ""
// and is never registered or decoded (removes decode-fail flutter).
func TestImageLifecycle_MissingFileNotDiscovered(t *testing.T) {
	dir := t.TempDir()
	// Note: no file written — resolveEmbed's exist-check fails, so it is skipped.
	m := docEditor(t, dir, "intro\n![alt](assets/missing.png)\noutro")

	m, cmd := m.discoverNewImages()
	if cmd != nil {
		t.Error("missing image file must not produce a decode Cmd")
	}
	if _, ok := m.images.get("assets/missing.png"); ok {
		t.Error("missing image file must not be registered")
	}
}

// TestImageLifecycle_DecodeErrorFallsBack verifies that a file which exists but
// fails to decode (corrupt / non-image bytes) transitions to the failed state
// and does not reserve rows.
func TestImageLifecycle_DecodeErrorFallsBack(t *testing.T) {
	dir := t.TempDir()
	// Write a present-but-corrupt "image" so resolveEmbed resolves it but decode fails.
	corrupt := filepath.Join(dir, "assets", "corrupt.png")
	if err := os.MkdirAll(filepath.Dir(corrupt), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(corrupt, []byte("not a real png"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := docEditor(t, dir, "intro\n![alt](assets/corrupt.png)\noutro")

	m, cmd := m.discoverNewImages()
	msgs := runCmd(t, cmd)
	errMsg, ok := firstMsg[ImageDecodeErrorMsg](msgs)
	if !ok {
		t.Fatalf("expected ImageDecodeErrorMsg, got %+v", msgs)
	}
	m, _ = m.Update(errMsg)
	if e, _ := m.images.get("assets/corrupt.png"); e.state != failed {
		t.Errorf("expected failed state, got %v", e.state)
	}
	// Failed image must not reserve rows.
	if m.snapshot.TotalRows != 3 {
		t.Errorf("failed image should not reserve rows, TotalRows=%d", m.snapshot.TotalRows)
	}
}

func TestImageLifecycle_RemoteURLSkipped(t *testing.T) {
	dir := t.TempDir()
	m := docEditor(t, dir, "intro\n![alt](https://example.com/x.png)\noutro")
	m, cmd := m.discoverNewImages()
	if cmd != nil {
		t.Error("remote image URLs must be skipped (no decode Cmd)")
	}
	if _, ok := m.images.get("https://example.com/x.png"); ok {
		t.Error("remote image should not be registered")
	}
}

func TestImageLifecycle_NonKittyNoDiscovery(t *testing.T) {
	dir := t.TempDir()
	writePNG(t, filepath.Join(dir, "assets", "x.png"), 100, 100)
	m := newTestEditor("")
	m.termCaps = terminal.TermCaps{} // not capable
	m = m.SetContent(filepath.Join(dir, "note.md"), []byte("intro\n![alt](assets/x.png)\noutro"))
	m = m.SetSize(80, 24)
	m, cmd := m.discoverNewImages()
	if cmd != nil {
		t.Error("non-capable terminal must not discover/decode images")
	}
}

// TestScrollToCursor_ImageRowsAboveCursor verifies the cursor's display row
// accounts for image-reserved rows above it (Finding 1 / WP4 step 4).
func TestScrollToCursor_ImageRowsAboveCursor(t *testing.T) {
	dir := t.TempDir()
	m := docEditor(t, dir, "![alt](assets/x.png)\nline1\nline2\nline3")
	// Small viewport to force scrolling. Register the live image AFTER SetSize
	// so resizeImages (which clamps rows to the viewport) does not shrink it.
	m = m.SetSize(80, 4)
	m.images = m.images.upsert(imageEntry{
		path: "assets/x.png", absPath: filepath.Join(dir, "assets/x.png"),
		id: 0x112233, cols: 6, rows: 5, pxW: 48, pxH: 80, state: live,
	})

	// Cursor on line 2 (third model line). Re-sync so the off-cursor image on
	// line 0 renders (and expands) rather than reveals.
	lineStart := len("![alt](assets/x.png)\nline1\n")
	m = setCursor(m, lineStart)
	m = m.syncDisplay()
	m = m.scrollToCursor()

	// In expanded space line 2 starts at row 5 (image) + 1 (line1) = 6.
	wantRow := m.snapshot.ModelLineToFirstRow(2)
	if wantRow != 6 {
		t.Fatalf("expected line 2 first expanded row 6, got %d", wantRow)
	}
	contentH := m.contentHeight()
	if wantRow < m.viewport.TopRow || wantRow >= m.viewport.TopRow+contentH {
		t.Errorf("cursor display row %d not visible in [%d,%d)", wantRow, m.viewport.TopRow, m.viewport.TopRow+contentH)
	}
}

// TestScrollToCursor_NoJumpOnImageLine verifies moving the cursor onto an image
// line then off does not jump the viewport when everything already fits.
func TestScrollToCursor_NoJumpOnImageLine(t *testing.T) {
	dir := t.TempDir()
	m := docEditor(t, dir, "line0\n![alt](assets/x.png)\nline2")
	m.images = m.images.upsert(imageEntry{
		path: "assets/x.png", absPath: filepath.Join(dir, "assets/x.png"),
		id: 0x445566, cols: 6, rows: 3, pxW: 48, pxH: 48, state: live,
	})
	m = m.SetSize(80, 40) // viewport larger than content
	m = m.syncDisplay()

	m = setCursor(m, 0) // line 0
	m = m.scrollToCursor()
	top0 := m.viewport.TopRow

	// Move onto the image line (reveals -> collapses to 1 row).
	m = setCursor(m, len("line0\n")+1)
	m = m.syncDisplay()
	m = m.scrollToCursor()
	topImg := m.viewport.TopRow

	// Move off, back to line 0.
	m = setCursor(m, 0)
	m = m.syncDisplay()
	m = m.scrollToCursor()
	topBack := m.viewport.TopRow

	if top0 != 0 || topImg != 0 || topBack != 0 {
		t.Errorf("viewport jumped: top0=%d topImg=%d topBack=%d (want all 0)", top0, topImg, topBack)
	}
}

// TestInlinePlacement_ViewContainsEscapes verifies that View() embeds iTerm2
// image escape sequences directly in its output when live images are visible.
// This ensures atomic rendering: no separate Cmd or TTY write is needed.
func TestInlinePlacement_ViewContainsEscapes(t *testing.T) {
	dir := t.TempDir()
	writePNG(t, filepath.Join(dir, "assets", "x.png"), 80, 80)

	m := newTestEditor("")
	m.termCaps = terminal.TermCaps{GraphicsProtocol: terminal.GraphicsWezTerm, TrueColor: true}
	m = m.SetContent(filepath.Join(dir, "note.md"), []byte("line0\n![alt](assets/x.png)\nline2"))
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m = m.SetOffset(0, 0)

	// Discover and decode.
	m, cmd := m.discoverNewImages()
	msgs := runCmd(t, cmd)
	decoded, ok := firstMsg[ImageDecodedMsg](msgs)
	if !ok {
		t.Fatal("expected ImageDecodedMsg")
	}
	m, cmd = m.Update(decoded)
	// Encode Cmd will produce ImageEncodedMsg (iTerm2 path).
	msgs = runCmd(t, cmd)
	encoded, ok := firstMsg[ImageEncodedMsg](msgs)
	if !ok {
		t.Fatalf("expected ImageEncodedMsg, got %+v", msgs)
	}
	m, _ = m.Update(encoded)

	// Now the image is live with iterm2Slices. InlineImagePlacements() should
	// contain the escape positioning sequences (appended at workspace level).
	seq := m.InlineImagePlacements()
	if !strings.Contains(seq, "\0337") {
		t.Error("InlineImagePlacements() should contain DECSC (cursor-save) escape")
	}
	if !strings.Contains(seq, "\0338") {
		t.Error("InlineImagePlacements() should contain DECRC (cursor-restore) escape")
	}
}

// TestInlinePlacement_ScrollChangesViewAtomically verifies that after mouse wheel
// scroll, the new View() contains correctly repositioned image escapes without
// needing any returned Cmd to do TTY writes.
func TestInlinePlacement_ScrollChangesViewAtomically(t *testing.T) {
	dir := t.TempDir()
	writePNG(t, filepath.Join(dir, "assets", "x.png"), 80, 80)

	// Create a document with enough lines that scrolling is needed.
	var lines []string
	for i := 0; i < 30; i++ {
		lines = append(lines, fmt.Sprintf("line%d", i))
	}
	// Place image at line 15 so it scrolls in/out of viewport.
	lines[15] = "![alt](assets/x.png)"
	content := strings.Join(lines, "\n")

	m := newTestEditor("")
	m.termCaps = terminal.TermCaps{GraphicsProtocol: terminal.GraphicsWezTerm, TrueColor: true}
	m = m.SetContent(filepath.Join(dir, "note.md"), []byte(content))
	m = m.SetSize(80, 10) // Small viewport
	m = m.SetFocused(true)
	m = m.SetOffset(0, 0)

	// Discover, decode, encode.
	m, cmd := m.discoverNewImages()
	msgs := runCmd(t, cmd)
	decoded, ok := firstMsg[ImageDecodedMsg](msgs)
	if !ok {
		t.Fatal("expected ImageDecodedMsg")
	}
	m, cmd = m.Update(decoded)
	msgs = runCmd(t, cmd)
	encoded, ok := firstMsg[ImageEncodedMsg](msgs)
	if !ok {
		t.Fatalf("expected ImageEncodedMsg, got %+v", msgs)
	}
	m, _ = m.Update(encoded)

	// Image is around line 15; scroll down to bring it into view.
	m.viewport.TopRow = 14
	seq1 := m.InlineImagePlacements()
	if !strings.Contains(seq1, "\0337") {
		t.Error("InlineImagePlacements() at TopRow=14 should contain inline image escapes")
	}

	// Scroll down by 1 more — image still visible but at different screen row.
	m.viewport.TopRow = 15
	seq2 := m.InlineImagePlacements()
	// The image starts at line 15 (row 0 in expanded space relative to image start).
	// At TopRow=14, it renders at screen row 1+; at TopRow=15, it renders at row 0+.
	// Both should have escape sequences, but with different row numbers.
	if seq1 == seq2 {
		t.Error("scrolling should change the image escape positions")
	}

	// Scroll image out of view entirely.
	m.viewport.TopRow = 0
	seq3 := m.InlineImagePlacements()
	if strings.Contains(seq3, "\0337") {
		t.Error("InlineImagePlacements() should be empty when image is out of viewport")
	}
}

// TestMouseWheel_NoReplotCmd verifies that handleMouseWheel does not return
// any image placement Cmd (placement is now done in View).
func TestMouseWheel_NoReplotCmd(t *testing.T) {
	dir := t.TempDir()
	writePNG(t, filepath.Join(dir, "assets", "x.png"), 80, 80)

	m := newTestEditor("")
	m.termCaps = terminal.TermCaps{GraphicsProtocol: terminal.GraphicsWezTerm, TrueColor: true}
	m = m.SetContent(filepath.Join(dir, "note.md"), []byte("line0\n![alt](assets/x.png)\nline2\nline3\nline4"))
	m = m.SetSize(80, 4)
	m = m.SetFocused(true)

	// Put a live image with iterm2 slices.
	m.images = m.images.upsert(imageEntry{
		path: "assets/x.png", absPath: filepath.Join(dir, "assets/x.png"),
		id: 0x112233, cols: 6, rows: 3, pxW: 48, pxH: 48, state: live,
		iterm2Slices: []string{"slice0", "slice1", "slice2"},
	})
	m = m.syncDisplay()

	// Scroll down.
	msg := tea.MouseWheelMsg{Button: tea.MouseWheelDown}
	m, cmd := m.handleMouseWheel(msg)
	// The only Cmd should be armImageTicks (a timer), not a writeTTY replot.
	if cmd != nil {
		msgs := runCmd(t, cmd)
		for _, msg := range msgs {
			if _, isPlaced := msg.(ImagePlacedMsg); isPlaced {
				t.Error("handleMouseWheel should not produce ImagePlacedMsg; View() handles placement")
			}
		}
	}
}
