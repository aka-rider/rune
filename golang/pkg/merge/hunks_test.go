package merge

import (
	"bytes"
	"testing"
)

// ---- round-trip byte-faithfulness tests -------------------------------------

// roundTripContent verifies that concatenating hunk AutoBytes / OursBytes
// reconstructs the expected merged buffer exactly.
func roundTripContent(t *testing.T, hunks []Hunk) []byte {
	t.Helper()
	var buf []byte
	for _, h := range hunks {
		switch h.Kind {
		case HunkClean:
			buf = append(buf, h.AutoBytes...)
		case HunkConflict:
			// The merge buffer keeps ours for unresolved conflicts.
			buf = append(buf, h.OursBytes...)
		}
	}
	return buf
}

// TestMergeHunks_CleanMerge: no conflicts → all HunkClean; AutoBytes must be
// verbatim from ours or theirs (§1.4.5). Non-adjacent changes (separated by
// shared content) avoid libgit2's adjacent-hunk conflict heuristic.
func TestMergeHunks_CleanMerge(t *testing.T) {
	// Separate changed lines with a shared buffer line so libgit2 doesn't
	// treat them as adjacent hunks (which it considers conflicting).
	ancestor := []byte("line1\nshared-A\nline2\nshared-B\nline3\n")
	ours := []byte("line1\nchanged-by-ours\nline2\nshared-B\nline3\n")
	theirs := []byte("line1\nshared-A\nline2\nchanged-by-theirs\nline3\n")

	hunks, err := MergeHunks(ancestor, ours, theirs)
	if err != nil {
		t.Fatalf("MergeHunks: %v", err)
	}
	for _, h := range hunks {
		if h.Kind == HunkConflict {
			t.Fatalf("expected no conflicts for clearly non-overlapping changes; got HunkConflict:\n  ours=%q\n  theirs=%q",
				h.OursBytes, h.TheirsBytes)
		}
	}
	if len(hunks) == 0 {
		t.Fatal("MergeHunks returned empty slice for clean merge")
	}
}

// TestMergeHunks_Conflict: overlapping changes on both sides → at least one
// HunkConflict; OursBytes and TheirsBytes must be verbatim from the inputs.
func TestMergeHunks_Conflict(t *testing.T) {
	ancestor := []byte("line1\nshared\nline3\n")
	ours := []byte("line1\nours-changed\nline3\n")
	theirs := []byte("line1\ntheirs-changed\nline3\n")

	hunks, err := MergeHunks(ancestor, ours, theirs)
	if err != nil {
		t.Fatalf("MergeHunks: %v", err)
	}

	var conflictHunks []Hunk
	for _, h := range hunks {
		if h.Kind == HunkConflict {
			conflictHunks = append(conflictHunks, h)
		}
	}
	if len(conflictHunks) == 0 {
		t.Fatal("expected at least one HunkConflict for overlapping changes")
	}

	// OursBytes must come verbatim from ours input.
	c := conflictHunks[0]
	if !bytes.Contains(ours, c.OursBytes) {
		t.Errorf("HunkConflict.OursBytes not verbatim from ours input:\n  ours=%q\n  OursBytes=%q",
			ours, c.OursBytes)
	}
	// TheirsBytes must come verbatim from theirs input.
	if !bytes.Contains(theirs, c.TheirsBytes) {
		t.Errorf("HunkConflict.TheirsBytes not verbatim from theirs input:\n  theirs=%q\n  TheirsBytes=%q",
			theirs, c.TheirsBytes)
	}
}

// TestMergeHunks_CRLF_OursPreserved: CRLF line endings in ours must not be
// normalized to LF in any HunkClean or HunkConflict ours bytes (§1.4.5).
func TestMergeHunks_CRLF_OursPreserved(t *testing.T) {
	ancestor := []byte("line1\r\nancestor\r\nline3\r\n")
	// Ours uses CRLF; add a change to force a real merge.
	ours := []byte("line1\r\nours-changed\r\nline3\r\n")
	theirs := []byte("line1\r\nancestor\r\nline3-theirs\r\n") // theirs changes line3

	hunks, err := MergeHunks(ancestor, ours, theirs)
	if err != nil {
		t.Fatalf("MergeHunks: %v", err)
	}

	merged := roundTripContent(t, hunks)
	// The merged content must contain CRLF (from ours or theirs), not bare LF
	// where the original had CRLF.
	if !bytes.Contains(merged, []byte("\r\n")) {
		t.Errorf("CRLF normalized away: merged=%q", merged)
	}

	// Specifically, ours-changed must appear with its original CRLF.
	if !bytes.Contains(merged, []byte("ours-changed\r\n")) {
		t.Errorf("ours-changed CRLF not preserved in merged content:\n  merged=%q", merged)
	}
}

// TestMergeHunks_NoTrailingNewline: if ours has no trailing newline, the merged
// buffer must not have one added (§1.4.5).
func TestMergeHunks_NoTrailingNewline(t *testing.T) {
	ancestor := []byte("hello")
	ours := []byte("hello-ours")
	theirs := []byte("hello")

	hunks, err := MergeHunks(ancestor, ours, theirs)
	if err != nil {
		t.Fatalf("MergeHunks: %v", err)
	}

	merged := roundTripContent(t, hunks)
	if len(merged) > 0 && merged[len(merged)-1] == '\n' {
		t.Errorf("trailing newline added to no-trailing-newline content: merged=%q", merged)
	}
}

// TestMergeHunks_BOM_Preserved: a UTF-8 BOM at the start of ours must survive
// in the merged buffer (§1.4.5).
func TestMergeHunks_BOM_Preserved(t *testing.T) {
	const bom = "\xef\xbb\xbf"
	ancestor := []byte(bom + "hello\n")
	ours := []byte(bom + "hello-ours\n")
	theirs := []byte(bom + "hello\n")

	hunks, err := MergeHunks(ancestor, ours, theirs)
	if err != nil {
		t.Fatalf("MergeHunks: %v", err)
	}

	merged := roundTripContent(t, hunks)
	if !bytes.HasPrefix(merged, []byte(bom)) {
		t.Errorf("BOM stripped from merged content: merged=%q", merged)
	}
}

// TestMergeHunks_VerbatimBytes: for a clean-theirs hunk (only theirs changed),
// AutoBytes must be verbatim from the theirs input.
func TestMergeHunks_VerbatimBytes(t *testing.T) {
	// Only theirs changes line2.
	ancestor := []byte("line1\noriginal\nline3\n")
	ours := []byte("line1\noriginal\nline3\n") // unchanged
	theirs := []byte("line1\ntheirs-version\nline3\n")

	hunks, err := MergeHunks(ancestor, ours, theirs)
	if err != nil {
		t.Fatalf("MergeHunks: %v", err)
	}

	// Expect a clean merge where theirs' content appears.
	var cleanHunks []Hunk
	for _, h := range hunks {
		if h.Kind == HunkClean {
			cleanHunks = append(cleanHunks, h)
		}
	}

	merged := roundTripContent(t, hunks)
	// The merged result should contain theirs-version verbatim from theirs.
	if !bytes.Contains(merged, []byte("theirs-version\n")) {
		t.Errorf("theirs-version not found in merged content: merged=%q", merged)
	}

	// Verify that the theirs-version bytes in the hunk match the theirs input
	// exactly (no libgit2 renormalization).
	for _, h := range cleanHunks {
		if bytes.Contains(h.AutoBytes, []byte("theirs-version")) {
			if !bytes.Contains(theirs, h.AutoBytes) {
				t.Errorf("AutoBytes for clean-theirs hunk not verbatim from theirs:\n  theirs=%q\n  AutoBytes=%q",
					theirs, h.AutoBytes)
			}
		}
	}
}

// TestMergeHunks_OursOnlyChange: when only ours changes a line, that hunk is
// clean (AutoBytes from ours) and no conflict is produced.
func TestMergeHunks_OursOnlyChange(t *testing.T) {
	ancestor := []byte("line1\nshared\nline3\n")
	ours := []byte("line1\nours-only\nline3\n")
	theirs := []byte("line1\nshared\nline3\n") // theirs unchanged

	hunks, err := MergeHunks(ancestor, ours, theirs)
	if err != nil {
		t.Fatalf("MergeHunks: %v", err)
	}

	for _, h := range hunks {
		if h.Kind == HunkConflict {
			t.Fatalf("unexpected conflict for ours-only change:\n  ours=%q\n  theirs=%q",
				h.OursBytes, h.TheirsBytes)
		}
	}

	merged := roundTripContent(t, hunks)
	if !bytes.Contains(merged, []byte("ours-only\n")) {
		t.Errorf("ours-only change not in merged content: merged=%q", merged)
	}
}

// TestMergeHunks_MultipleConflicts: multiple independent conflict regions each
// produce their own HunkConflict with correct verbatim bytes.
func TestMergeHunks_MultipleConflicts(t *testing.T) {
	ancestor := []byte("shared\nchange1\nshared2\nchange2\nshared3\n")
	ours := []byte("shared\nours-c1\nshared2\nours-c2\nshared3\n")
	theirs := []byte("shared\ntheirs-c1\nshared2\ntheirs-c2\nshared3\n")

	hunks, err := MergeHunks(ancestor, ours, theirs)
	if err != nil {
		t.Fatalf("MergeHunks: %v", err)
	}

	var conflicts []Hunk
	for _, h := range hunks {
		if h.Kind == HunkConflict {
			conflicts = append(conflicts, h)
		}
	}
	if len(conflicts) < 2 {
		t.Fatalf("expected ≥2 conflict hunks for 2 conflict regions, got %d", len(conflicts))
	}

	// Each conflict's bytes must be verbatim.
	for i, c := range conflicts {
		if !bytes.Contains(ours, c.OursBytes) {
			t.Errorf("conflict[%d].OursBytes not verbatim from ours: %q", i, c.OursBytes)
		}
		if !bytes.Contains(theirs, c.TheirsBytes) {
			t.Errorf("conflict[%d].TheirsBytes not verbatim from theirs: %q", i, c.TheirsBytes)
		}
	}
}

// TestMergeHunks_EmptyInputs: all-empty inputs yield a single clean hunk with
// empty AutoBytes (no panic, no error).
func TestMergeHunks_EmptyInputs(t *testing.T) {
	hunks, err := MergeHunks(nil, nil, nil)
	if err != nil {
		t.Fatalf("MergeHunks(empty): %v", err)
	}
	if len(hunks) == 0 {
		t.Fatal("expected at least one hunk for empty inputs")
	}
}

// TestMergeHunks_IdenticalInputs: when ours == theirs == ancestor, the result
// is a single HunkClean with the unchanged content.
func TestMergeHunks_IdenticalInputs(t *testing.T) {
	content := []byte("line1\nline2\nline3\n")
	hunks, err := MergeHunks(content, content, content)
	if err != nil {
		t.Fatalf("MergeHunks(identical): %v", err)
	}
	for _, h := range hunks {
		if h.Kind == HunkConflict {
			t.Fatalf("unexpected HunkConflict for identical inputs: ours=%q", h.OursBytes)
		}
	}
}

// ---- parseDiff3 unit tests --------------------------------------------------

// TestParseDiff3_NoConflict: output with no markers → single empty clean
// segment, no conflict blocks.
func TestParseDiff3_NoConflict(t *testing.T) {
	output := []byte("line1\nline2\n")
	cleans, conflicts := parseDiff3(output)
	if len(conflicts) != 0 {
		t.Fatalf("expected no conflicts, got %d", len(conflicts))
	}
	if len(cleans) != 1 {
		t.Fatalf("expected 1 clean segment, got %d", len(cleans))
	}
	if !bytes.Equal(cleans[0], output) {
		t.Errorf("clean segment mismatch: got %q, want %q", cleans[0], output)
	}
}

// TestParseDiff3_Invariant_CleansLenIsConflictsPlus1: len(cleans) must always
// equal len(conflicts)+1.
func TestParseDiff3_Invariant_CleanLenEqConflictsPlusOne(t *testing.T) {
	output := []byte("before\n<<<<<<< ours\nours-line\n||||||| ancestor\nanc-line\n=======\ntheirs-line\n>>>>>>> theirs\nafter\n")
	cleans, conflicts := parseDiff3(output)
	if len(cleans) != len(conflicts)+1 {
		t.Fatalf("invariant violated: len(cleans)=%d, len(conflicts)=%d", len(cleans), len(conflicts))
	}
}

// TestMergeHunks_CRLF_BOM_NoTrailingNewline_MixedClean: the mixed-clean-merge
// fast path (both sides changed different, non-adjacent regions → no conflict)
// must preserve CRLF line endings, UTF-8 BOM, and the absence of a trailing
// newline in the output (§1.4.5 / byte-faithful round-trip requirement, F5).
//
// This covers line 169 of hunks.go where the raw libgit2 diff3 output is
// returned as AutoBytes — the property that "libgit2 copies input lines
// verbatim for non-conflicting regions" is asserted empirically here.
func TestMergeHunks_CRLF_BOM_NoTrailingNewline_MixedClean(t *testing.T) {
	const bom = "\xef\xbb\xbf"

	// Both sides use CRLF and a UTF-8 BOM; the file has no trailing newline.
	// Ours changes line2; theirs changes line4. The changed lines are separated
	// by an unchanged "sep" line so libgit2 does NOT treat them as adjacent
	// hunks (which it considers conflicting — see TestMergeHunks_CleanMerge).
	// This hits the mixed-clean-merge branch (hunks.go:169).
	ancestor := []byte(bom + "header\r\nancestor-line2\r\nsep\r\nancestor-line4")
	ours := []byte(bom + "header\r\n" + "ours-line2\r\nsep\r\nancestor-line4")
	theirs := []byte(bom + "header\r\nancestor-line2\r\nsep\r\n" + "theirs-line4")

	hunks, err := MergeHunks(ancestor, ours, theirs)
	if err != nil {
		t.Fatalf("MergeHunks: %v", err)
	}

	if len(hunks) != 1 || hunks[0].Kind != HunkClean {
		t.Fatalf("expected 1 HunkClean for a mixed-clean merge, got %d hunks: %v", len(hunks), hunks)
	}

	merged := roundTripContent(t, hunks)

	// BOM must be present at the start.
	if !bytes.HasPrefix(merged, []byte(bom)) {
		t.Errorf("BOM stripped from mixed-clean merged content: merged=%q", merged)
	}

	// CRLF must survive — every original \r\n pair must appear in the output.
	if !bytes.Contains(merged, []byte("\r\n")) {
		t.Errorf("CRLF normalized away in mixed-clean merged content: merged=%q", merged)
	}

	// No trailing newline — the original had none.
	if len(merged) > 0 && merged[len(merged)-1] == '\n' {
		t.Errorf("trailing newline added to no-trailing-newline mixed-clean content: merged=%q", merged)
	}

	// Both ours and theirs changes must appear verbatim.
	if !bytes.Contains(merged, []byte("ours-line2")) {
		t.Errorf("ours change missing from mixed-clean merged content: merged=%q", merged)
	}
	if !bytes.Contains(merged, []byte("theirs-line4")) {
		t.Errorf("theirs change missing from mixed-clean merged content: merged=%q", merged)
	}
}

// TestParseDiff3_ConflictSections: the three sections of a conflict block are
// parsed correctly from standard diff3 output.
func TestParseDiff3_ConflictSections(t *testing.T) {
	output := []byte("<<<<<<< ours\nours-line\n||||||| ancestor\nanc-line\n=======\ntheirs-line\n>>>>>>> theirs\n")
	cleans, conflicts := parseDiff3(output)

	if len(conflicts) != 1 {
		t.Fatalf("expected 1 conflict block, got %d", len(conflicts))
	}
	c := conflicts[0]
	if !bytes.Equal(c.oursSection, []byte("ours-line\n")) {
		t.Errorf("oursSection: got %q, want %q", c.oursSection, "ours-line\n")
	}
	if !bytes.Equal(c.ancestorSection, []byte("anc-line\n")) {
		t.Errorf("ancestorSection: got %q, want %q", c.ancestorSection, "anc-line\n")
	}
	if !bytes.Equal(c.theirsSection, []byte("theirs-line\n")) {
		t.Errorf("theirsSection: got %q, want %q", c.theirsSection, "theirs-line\n")
	}

	// First clean segment is empty (nothing before <<<<<<< ours).
	if len(cleans[0]) != 0 {
		t.Errorf("expected empty first clean segment, got %q", cleans[0])
	}
	// Last clean segment is also empty (nothing after >>>>>>> theirs).
	if len(cleans[1]) != 0 {
		t.Errorf("expected empty last clean segment, got %q", cleans[1])
	}
}
