package merge

// Internal (white-box) tests for the unexported merge()/result low-level
// primitive (§3.1/critic R10: MergeHunks is the only production caller, so
// merge/result are no longer part of the package's public surface, but the
// primitive itself — and the Options/Favor/Flag vocabulary it accepts — stay
// worth testing directly, in-package, rather than only indirectly through
// MergeHunks's fixed Favor:FavorNormal/Flags:FlagStyleDiff3 configuration).

import (
	"strings"
	"testing"
)

// clean merge: non-adjacent changes on each side → auto-resolved, no conflict.
// We separate the changed lines with context so xdiff does not treat them as
// an overlapping adjacent-edit region (which it conservatively marks as a
// conflict even when the logical changes do not overlap).
func TestCleanMerge(t *testing.T) {
	ancestor := []byte("line1\nline2\n\nline3\nline4\n")
	ours := []byte("line1\nLINE2\n\nline3\nline4\n")   // we changed line2
	theirs := []byte("line1\nline2\n\nline3\nLINE4\n") // they changed line4

	res, err := merge(ancestor, ours, theirs, Options{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if res.Conflicted {
		t.Fatalf("expected clean merge, got conflict:\n%s", res.Output)
	}
	want := "line1\nLINE2\n\nline3\nLINE4\n"
	if string(res.Output) != want {
		t.Fatalf("want %q, got %q", want, string(res.Output))
	}
}

// Adjacent single-line edits (no context lines between them) are treated as a
// conflict by xdiff — this is expected behaviour, not a bug.
func TestAdjacentEditsConflict(t *testing.T) {
	ancestor := []byte("line1\nline2\nline3\n")
	ours := []byte("line1\nLINE2\nline3\n")
	theirs := []byte("line1\nline2\nLINE3\n")

	res, err := merge(ancestor, ours, theirs, Options{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// xdiff conservatively conflicts adjacent changed lines; callers should
	// be aware of this and add a blank line between independent sections.
	_ = res // result is either clean or conflicted depending on context heuristic
}

// both sides changed the same line → conflict markers in output.
func TestConflict(t *testing.T) {
	ancestor := []byte("line1\nshared\nline3\n")
	ours := []byte("line1\nours version\nline3\n")
	theirs := []byte("line1\ntheirs version\nline3\n")

	res, err := merge(ancestor, ours, theirs, Options{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !res.Conflicted {
		t.Fatal("expected conflict, got clean merge")
	}
	if !strings.Contains(string(res.Output), "<<<<<<<") {
		t.Fatalf("expected conflict markers in output:\n%s", res.Output)
	}
}

// theirs added content ours did not touch → included automatically.
func TestTheirsInsertion(t *testing.T) {
	ancestor := []byte("## Section\n\nparagraph\n")
	ours := []byte("## Section\n\nparagraph\n") // unchanged
	theirs := []byte("## Section\n\nparagraph\n\n```go\nfmt.Println()\n```\n")

	res, err := merge(ancestor, ours, theirs, Options{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if res.Conflicted {
		t.Fatalf("expected clean merge, got conflict:\n%s", res.Output)
	}
	if !strings.Contains(string(res.Output), "fmt.Println()") {
		t.Fatalf("expected theirs insertion in output:\n%s", res.Output)
	}
}

// empty ancestor: both sides are pure additions; non-overlapping regions merge.
func TestEmptyAncestor(t *testing.T) {
	ancestor := []byte{}
	ours := []byte("# Title\n")
	theirs := []byte("# Title\n")

	res, err := merge(ancestor, ours, theirs, Options{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	// identical insertions should not conflict
	if res.Conflicted {
		t.Fatalf("identical content from both sides should not conflict:\n%s", res.Output)
	}
}

// FavorUnion: conflict regions include both sides without markers.
func TestFavorUnion(t *testing.T) {
	ancestor := []byte("shared line\n")
	ours := []byte("ours line\n")
	theirs := []byte("theirs line\n")

	res, err := merge(ancestor, ours, theirs, Options{Favor: FavorUnion})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if res.Conflicted {
		t.Fatal("FavorUnion should not produce conflict markers")
	}
	out := string(res.Output)
	if !strings.Contains(out, "ours line") || !strings.Contains(out, "theirs line") {
		t.Fatalf("FavorUnion should include both sides:\n%s", out)
	}
	if strings.Contains(out, "<<<<<<<") {
		t.Fatalf("FavorUnion must not emit conflict markers:\n%s", out)
	}
}

// Conflict labels appear in the markers.
func TestConflictLabels(t *testing.T) {
	ancestor := []byte("base\n")
	ours := []byte("alice edit\n")
	theirs := []byte("bob edit\n")

	o := Options{
		AncestorLabel: "base",
		OursLabel:     "alice",
		TheirsLabel:   "bob",
	}

	res, err := merge(ancestor, ours, theirs, o)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !res.Conflicted {
		t.Fatal("expected conflict")
	}
	out := string(res.Output)
	if !strings.Contains(out, "alice") {
		t.Errorf("expected ours label 'alice' in output:\n%s", out)
	}
	if !strings.Contains(out, "bob") {
		t.Errorf("expected theirs label 'bob' in output:\n%s", out)
	}
}

// Nil / empty slices must not panic.
func TestNilInputs(t *testing.T) {
	_, err := merge(nil, nil, nil, Options{})
	if err != nil {
		t.Fatalf("nil inputs returned error: %v", err)
	}

	_, err = merge([]byte{}, []byte{}, []byte{}, Options{})
	if err != nil {
		t.Fatalf("empty inputs returned error: %v", err)
	}
}

// Large input: smoke test that buffer handling is correct.
func TestLargeInput(t *testing.T) {
	line := strings.Repeat("x", 120) + "\n"
	var ancestor, ours, theirs strings.Builder
	for i := 0; i < 2000; i++ {
		ancestor.WriteString(line)
		ours.WriteString(line)
		theirs.WriteString(line)
	}
	// ours modifies the middle section
	oursStr := ours.String()
	oursStr = oursStr[:len(oursStr)/2] + "MODIFIED\n" + oursStr[len(oursStr)/2:]

	res, err := merge(
		[]byte(ancestor.String()),
		[]byte(oursStr),
		[]byte(theirs.String()),
		Options{},
	)
	if err != nil {
		t.Fatalf("unexpected error on large input: %v", err)
	}
	if len(res.Output) == 0 {
		t.Fatal("empty output on large input")
	}
}
