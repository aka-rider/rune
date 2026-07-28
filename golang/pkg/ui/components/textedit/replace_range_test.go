package textedit_test

import (
	"strings"
	"testing"
	"unicode/utf8"

	"rune/pkg/ui/components/textedit"
)

// mustReplaceRange asserts the happy path: the range fits the buffer and the
// edit succeeds.
func mustReplaceRange(t *testing.T, m textedit.Model, start, end int, text string) textedit.Model {
	t.Helper()
	m, err := m.ReplaceRange(start, end, text)
	if err != nil {
		t.Fatalf("ReplaceRange(%d,%d,%q): unexpected error: %v", start, end, text, err)
	}
	return m
}

// TestReplaceRangeCursorAtByteOffsetMultibyte is the regression test for the
// §1.5 rune-count-as-byte-offset bug: ReplaceRange used to place the cursor at
// start+utf8.RuneCountInString(text) instead of start+len(text). Any inserted
// text containing a multibyte rune (CJK, emoji, accents) then desynced the
// cursor from the buffer's byte offsets — reachable via merge [D]iscard,
// dictation, and image/link insert (all funnel through ReplaceRange, §12).
func TestReplaceRangeCursorAtByteOffsetMultibyte(t *testing.T) {
	m := newModel("")
	text := "héllo 世界 🎉" // multibyte runes: é(2), 世/界(3 each), 🎉(4)
	if utf8.RuneCountInString(text) == len(text) {
		t.Fatalf("test fixture %q must contain multibyte runes (rune count == byte len)", text)
	}
	m = mustReplaceRange(t, m, 0, 0, text)

	wantOffset := len(text) // BYTES, per §1.5 — not utf8.RuneCountInString(text)
	if got := m.CursorOffset(); got != wantOffset {
		t.Errorf("cursor offset after multibyte ReplaceRange: got %d, want %d (byte length of %q, rune count %d)",
			got, wantOffset, text, utf8.RuneCountInString(text))
	}
	// The cursor offset must land on a UTF-8 rune boundary — a corrupted
	// offset (rune count instead of byte length) can split a multibyte
	// sequence, and a subsequent edit at that offset would corrupt the buffer.
	if !utf8.ValidString(m.Content()[:m.CursorOffset()]) {
		t.Errorf("content up to cursor offset %d is not valid UTF-8: %q", m.CursorOffset(), m.Content()[:m.CursorOffset()])
	}
}

// TestReplaceRangeCursorAtByteOffsetASCII guards the trivial ASCII case where
// byte length and rune count coincide (so the pre-fix bug would not have been
// caught by ASCII-only coverage).
func TestReplaceRangeCursorAtByteOffsetASCII(t *testing.T) {
	m := newModel("")
	m = mustReplaceRange(t, m, 0, 0, "hello")
	if got, want := m.CursorOffset(), len("hello"); got != want {
		t.Errorf("cursor offset after ASCII ReplaceRange: got %d, want %d", got, want)
	}
}

// TestReplaceRangeMidBufferMultibyte verifies start+len(text) placement when
// replacing a non-empty range in the middle of existing multibyte content.
func TestReplaceRangeMidBufferMultibyte(t *testing.T) {
	m := newModel("prefix-OLD-suffix")
	start := strings.Index(m.Content(), "OLD")
	end := start + len("OLD")
	replacement := "新内容🎉"
	m = mustReplaceRange(t, m, start, end, replacement)

	wantOffset := start + len(replacement)
	if got := m.CursorOffset(); got != wantOffset {
		t.Errorf("cursor offset after mid-buffer multibyte ReplaceRange: got %d, want %d", got, wantOffset)
	}
	wantContent := "prefix-新内容🎉-suffix"
	if got := m.Content(); got != wantContent {
		t.Errorf("content after mid-buffer multibyte ReplaceRange: got %q, want %q", got, wantContent)
	}
}

// TestReplaceRangeOutOfBoundsSurfacesError verifies the §1.3 contract: an
// out-of-bounds ReplaceRange returns a non-nil error and leaves the buffer
// UNCHANGED (never a silent drop) — the fix for the previous
// `if err != nil { return m }` silent swallow.
func TestReplaceRangeOutOfBoundsSurfacesError(t *testing.T) {
	m := newModel("hi") // length 2
	got, err := m.ReplaceRange(100, 200, "x")
	if err == nil {
		t.Fatal("ReplaceRange out-of-bounds: expected error, got nil")
	}
	if got.Content() != "hi" {
		t.Errorf("ReplaceRange out-of-bounds: buffer mutated to %q, want unchanged %q", got.Content(), "hi")
	}
}
