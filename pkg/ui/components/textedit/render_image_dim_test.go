package textedit_test

import (
	"testing"

	"rune/pkg/editor/display"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// declineAllImageRows is an textedit.ImageRowFunc that always reports
// ok=false — every line falls through to ordinary span rendering, exactly
// as if imageRow had never recognized the line at all.
func declineAllImageRows(l display.DisplayLine) ([]textedit.Cell, bool) {
	return nil, false
}

// TestRenderView_ImageRowDeclineStillDims is B4's regression test: dimming
// used to be keyed on DisplayLine.ImagePath == "" — a hook that DECLINES a
// row (ok=false, falling through to ordinary span rendering) left ImagePath
// non-empty even though the row was rendered as plain text, so it wrongly
// escaped dimming. The fix keys dimming on whether imageRow ACTUALLY handled
// the row (ok=true), so a declined image row must dim exactly like an
// ordinary unfocused line with identical rendered text.
//
// Driven entirely through the real SetContent/SetImageDims pipeline (no
// synthetic DisplaySnapshot injection — textedit.SetSnapshot was deleted as
// dead production surface, B1): a standalone image line whose alt text is
// "hello" renders, concealed and unfocused, to the exact same span text as a
// plain "hello" paragraph — defaultCellBuilder styles every span identically
// regardless of TokenKind (§12), so the only variable under test is whether
// DisplayLine.ImagePath being non-empty (set by ExpandImageRows once
// SetImageDims reports more than one row for the image) escapes dimming on
// its own, independent of imageRow's ok result. Comparing only row 0 (H:1)
// avoids the image's reserved continuation row, which has no ordinary-line
// counterpart.
func TestRenderView_ImageRowDeclineStillDims(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	ordinary := textedit.New(keys, st).
		SetContent("hello").
		SetRect(textedit.Rect{W: 20, H: 1}).
		SetFocused(false)
	ordinaryView := ordinary.RenderView(nil, declineAllImageRows)

	declinedImage := textedit.New(keys, st).
		SetContent("![hello](some/image.png)").
		SetRect(textedit.Rect{W: 20, H: 1}).
		SetFocused(false).
		SetImageDims(map[string]display.ImageDims{"some/image.png": {Cols: 5, Rows: 2}})
	declinedImageView := declinedImage.RenderView(nil, declineAllImageRows)

	if declinedImageView != ordinaryView {
		t.Fatalf("a declined (ok=false) image row must dim exactly like an ordinary line:\nordinary:       %q\ndeclined-image: %q",
			ordinaryView, declinedImageView)
	}
}
