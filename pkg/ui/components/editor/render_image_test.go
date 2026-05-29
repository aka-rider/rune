package editor

import (
	"fmt"
	"strings"
	"testing"

	"rune/pkg/editor/display"
	"rune/pkg/imagekit"
	"rune/pkg/terminal"
)

const placeholderRune = "\U0010EEEE"

func kittyCaps() terminal.TermCaps {
	return terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true}
}

func TestImagePlaceholderCells_WidthAndCount(t *testing.T) {
	cells := imagePlaceholderCells(0x123456, 2, 7)
	if len(cells) != 7 {
		t.Fatalf("got %d cells, want 7", len(cells))
	}
	for i, c := range cells {
		if c.Width != 1 {
			t.Errorf("cell %d width=%d, want 1", i, c.Width)
		}
		if c.BufOffset != -1 {
			t.Errorf("cell %d BufOffset=%d, want -1", i, c.BufOffset)
		}
		if !strings.HasPrefix(c.Grapheme, placeholderRune) {
			t.Errorf("cell %d grapheme missing placeholder: %q", i, c.Grapheme)
		}
	}
}

func TestIDToColor_RoundTrip24Bit(t *testing.T) {
	for _, id := range []uint32{0x000001, 0xAABBCC, 0xFFFFFF, 0x1000000 | 0x0000FF} {
		c := idToColor(id)
		want := fmt.Sprintf("#%06X", id&0xFFFFFF)
		// lipgloss.Color renders to an SGR; compare hex via String if possible.
		if got := colorHex(c); got != want {
			t.Errorf("idToColor(%#x) hex=%s, want %s", id, got, want)
		}
	}
}

// colorHex extracts the #RRGGBB form from a color via its RGBA values.
func colorHex(c interface{ RGBA() (r, g, b, a uint32) }) string {
	r, g, b, _ := c.RGBA()
	return fmt.Sprintf("#%02X%02X%02X", r>>8, g>>8, b>>8)
}

// imageEditor builds a kitty-capable editor whose registry has a live image at
// the given raw path with the given cell footprint, then syncs display so the
// snapshot reserves rows. The cursor stays on line 0 (the "intro" line) so the
// image on line 1 stays Rendered (not revealed to source).
func imageEditor(t *testing.T, caps terminal.TermCaps, path string, cols, rows int) Model {
	t.Helper()
	m := newTestEditor("intro\n![alt](" + path + ")\noutro")
	m.termCaps = caps
	id := imagekit.AllocID(path)
	m.images = m.images.upsert(imageEntry{
		path:    path,
		absPath: path,
		id:      id,
		cols:    cols,
		rows:    rows,
		state:   live,
	})
	m = m.syncDisplay()
	return m
}

func TestView_KittyCapable_ContainsPlaceholderAndTruecolor(t *testing.T) {
	path := "assets/x.png"
	m := imageEditor(t, kittyCaps(), path, 6, 4)
	m = m.SetFocused(true)

	out := m.View()

	if !strings.Contains(out, placeholderRune) {
		t.Error("expected placeholder rune U+10EEEE in kitty-capable View output")
	}
	if !strings.Contains(out, "38;2;") {
		t.Error("expected a truecolor SGR (38;2;) for the image ID")
	}
	id := imagekit.AllocID(path)
	r, g, b := (id>>16)&0xFF, (id>>8)&0xFF, id&0xFF
	wantSGR := fmt.Sprintf("38;2;%d;%d;%d", r, g, b)
	if !strings.Contains(out, wantSGR) {
		t.Errorf("expected SGR %q for image id %#x in output", wantSGR, id)
	}
}

func TestView_RenderPurity(t *testing.T) {
	m := imageEditor(t, kittyCaps(), "assets/x.png", 6, 4)
	m = m.SetFocused(true)
	a := m.View()
	b := m.View()
	if a != b {
		t.Error("View() is not pure: two calls differ")
	}
}

func TestView_NonKitty_NoPlaceholder_FallbackAltText(t *testing.T) {
	path := "assets/x.png"
	// Non-kitty editor: registry has an entry but caps are empty.
	m := newTestEditor("![diagram](" + path + ")")
	m.termCaps = terminal.TermCaps{}
	m.images = m.images.upsert(imageEntry{path: path, absPath: path, id: 7, cols: 6, rows: 4, state: live})
	m = m.syncDisplay()
	m = m.SetFocused(true)

	out := m.View()
	if strings.Contains(out, placeholderRune) {
		t.Error("non-kitty View must not contain placeholder runes")
	}
	// Alt text fallback should be present.
	if !strings.Contains(out, "diagram") {
		t.Error("expected alt-text fallback 'diagram' in non-kitty output")
	}
}

func TestView_WezTerm_NoPlaceholder(t *testing.T) {
	path := "assets/x.png"
	m := newTestEditor("![alt](" + path + ")")
	m.termCaps = terminal.TermCaps{GraphicsProtocol: terminal.GraphicsWezTerm, TrueColor: true}
	m.images = m.images.upsert(imageEntry{path: path, absPath: path, id: 7, cols: 6, rows: 4, state: live})
	m = m.syncDisplay()
	m = m.SetFocused(true)

	out := m.View()
	if strings.Contains(out, placeholderRune) {
		t.Error("WezTerm must use the alt-text fallback, not placeholders (Finding 2)")
	}
}

func TestView_RowCountParity_NonKittyVsKitty(t *testing.T) {
	path := "assets/x.png"
	doc := "line1\n![alt](" + path + ")\nline3"

	nonKitty := newTestEditor(doc)
	nonKitty.termCaps = terminal.TermCaps{}
	nonKitty = nonKitty.syncDisplay()

	if nonKitty.snapshot.TotalRows != 3 {
		t.Errorf("non-kitty snapshot should keep 3 rows, got %d", nonKitty.snapshot.TotalRows)
	}

	// Sanity: ExpandImageRows for the non-kitty dimsFor is a no-op.
	out := display.ExpandImageRows(nonKitty.snapshot, nonKitty.imageDimsFor)
	if out.TotalRows != 3 {
		t.Errorf("non-kitty ExpandImageRows should not expand, got %d rows", out.TotalRows)
	}
}
