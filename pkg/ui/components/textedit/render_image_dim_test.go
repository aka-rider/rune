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

// snapshotWithLine builds a minimal one-line DisplaySnapshot; imagePath, when
// non-empty, marks the line as an image-embed candidate (DisplayLine.ImagePath).
func snapshotWithLine(text, imagePath string) display.DisplaySnapshot {
	return display.DisplaySnapshot{
		Lines: []display.DisplayLine{
			{
				Spans: []display.DisplaySpan{
					{Text: text, BufferStart: 0, BufferEnd: len(text)},
				},
				ImagePath: imagePath,
			},
		},
		TotalRows: 1,
	}
}

// TestRenderView_ImageRowDeclineStillDims is B4's regression test: dimming
// used to be keyed on DisplayLine.ImagePath == "" — a hook that DECLINES a
// row (ok=false, falling through to ordinary span rendering) left ImagePath
// non-empty even though the row was rendered as plain text, so it wrongly
// escaped dimming. The fix keys dimming on whether imageRow ACTUALLY handled
// the row (ok=true), so a declined image row must dim exactly like an
// ordinary unfocused line with identical text.
func TestRenderView_ImageRowDeclineStillDims(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	ordinary := textedit.New(keys, st).
		SetRect(textedit.Rect{W: 20, H: 3}).
		SetFocused(false).
		SetSnapshot(snapshotWithLine("hello", ""))
	ordinaryView := ordinary.RenderView(nil, declineAllImageRows)

	declinedImage := textedit.New(keys, st).
		SetRect(textedit.Rect{W: 20, H: 3}).
		SetFocused(false).
		SetSnapshot(snapshotWithLine("hello", "some/image.png"))
	declinedImageView := declinedImage.RenderView(nil, declineAllImageRows)

	if declinedImageView != ordinaryView {
		t.Fatalf("a declined (ok=false) image row must dim exactly like an ordinary line:\nordinary:       %q\ndeclined-image: %q",
			ordinaryView, declinedImageView)
	}
}
