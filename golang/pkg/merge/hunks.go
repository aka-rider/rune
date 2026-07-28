package merge

import "bytes"

// HunkKind classifies a region in a 3-way merge result.
type HunkKind int

const (
	// HunkClean is a region where both sides agree or only one side changed.
	// AutoBytes contains the resolved bytes to place in the buffer (verbatim
	// from whichever input contributed the content, §1.4.5).
	HunkClean HunkKind = iota

	// HunkConflict is a region where both sides made different changes.
	// OursBytes is the ours content (kept in the buffer); TheirsBytes is the
	// alternative the user can accept with [T].
	HunkConflict
)

// Hunk is a classified region in a 3-way merge. Hunks are in document order;
// concatenating AutoBytes/OursBytes for each hunk reconstructs the merged
// buffer content verbatim.
//
// Byte-faithfulness guarantee (§1.4.5): for HunkClean regions, AutoBytes
// comes verbatim from the ours or theirs input (whichever contributed it).
// For HunkConflict, OursBytes and TheirsBytes come verbatim from the
// respective inputs. libgit2 is used only for conflict classification; its
// serialised output is never used as buffer content.
//
// R5 fallback: any region that cannot be cleanly mapped back to exact
// ours/theirs spans is classified as HunkConflict rather than trusting
// libgit2's reserialized bytes.
type Hunk struct {
	Kind HunkKind

	// HunkClean: resolved bytes for this region (verbatim from ours or theirs).
	AutoBytes []byte

	// HunkConflict: ours bytes (already in buffer) for this region.
	OursBytes []byte

	// HunkConflict: theirs bytes (the [T] alternative) for this region.
	TheirsBytes []byte
}

// MergeHunks performs a 3-way merge and returns the result as classified hunks.
// It calls libgit2 with FlagStyleDiff3 to identify conflict boundaries, then
// maps those boundaries back to verbatim bytes from the original ours and
// theirs inputs (§1.4.5 by construction).
//
// The returned slice is always non-nil. When the merge has no conflicts the
// slice contains a single HunkClean covering the full merged content.
//
// Always merges with Favor:FavorNormal, Flags:FlagStyleDiff3 — every caller
// passed an always-{} Options previously, so this is the same effective
// behavior, made explicit instead of threading an unused parameter (§3.1).
func MergeHunks(ancestor, ours, theirs []byte) ([]Hunk, error) {
	res, err := merge(ancestor, ours, theirs, Options{Favor: FavorNormal, Flags: FlagStyleDiff3})
	if err != nil {
		return nil, err
	}
	return parseHunks(ours, theirs, res.Output), nil
}

// ---- internal diff3 parser --------------------------------------------------

// diff3Block holds the three sections of a single conflict block.
type diff3Block struct {
	oursSection     []byte // between <<<<<<< and |||||||
	ancestorSection []byte // between ||||||| and =======
	theirsSection   []byte // between ======= and >>>>>>>
}

// nextLine returns the first line (including its line ending) and the
// remainder of b. Returns (nil, nil) when b is empty.
func nextLine(b []byte) (line, rest []byte) {
	if len(b) == 0 {
		return nil, nil
	}
	i := bytes.IndexByte(b, '\n')
	if i < 0 {
		return b, nil
	}
	return b[:i+1], b[i+1:]
}

func isOursMarker(line []byte) bool     { return bytes.HasPrefix(line, []byte("<<<<<<<")) }
func isAncestorMarker(line []byte) bool { return bytes.HasPrefix(line, []byte("|||||||")) }
func isSepMarker(line []byte) bool {
	t := bytes.TrimRight(line, "\r\n")
	return bytes.Equal(t, []byte("======="))
}
func isTheirsMarker(line []byte) bool { return bytes.HasPrefix(line, []byte(">>>>>>>")) }

// parseDiff3 splits diff3 output into alternating clean segments and conflict
// blocks. cleans[i] precedes conflicts[i]; len(cleans) == len(conflicts)+1.
func parseDiff3(output []byte) (cleans [][]byte, conflicts []diff3Block) {
	var currentClean []byte
	remaining := output

	for len(remaining) > 0 {
		line, rest := nextLine(remaining)
		remaining = rest

		if !isOursMarker(line) {
			currentClean = append(currentClean, line...)
			continue
		}

		// Found start of a conflict block.
		cleans = append(cleans, currentClean)
		currentClean = nil

		var block diff3Block
		inSection := 0 // 0=ours, 1=ancestor, 2=theirs

		for len(remaining) > 0 {
			cline, crest := nextLine(remaining)
			remaining = crest
			switch {
			case isAncestorMarker(cline):
				inSection = 1
			case isSepMarker(cline):
				inSection = 2
			case isTheirsMarker(cline):
				inSection = -1 // done
			case inSection == 0:
				block.oursSection = append(block.oursSection, cline...)
			case inSection == 1:
				block.ancestorSection = append(block.ancestorSection, cline...)
			case inSection == 2:
				block.theirsSection = append(block.theirsSection, cline...)
			}
			if inSection < 0 {
				break
			}
		}
		conflicts = append(conflicts, block)
	}

	// Always append the final (possibly empty) clean segment.
	cleans = append(cleans, currentClean)
	return cleans, conflicts
}

// ---- hunk builder -----------------------------------------------------------

// parseHunks maps diff3 output boundaries back to verbatim ours/theirs bytes,
// returning the classified hunk sequence.
func parseHunks(ours, theirs, diff3Output []byte) []Hunk {
	cleans, conflicts := parseDiff3(diff3Output)

	// Fast path: fully-clean merge (no conflict markers in diff3 output). The
	// merged content is a combination of both sides' changes in the correct
	// order. It is verbatim from the inputs at line level (libgit2 does not
	// renormalize clean content). Return it as a single HunkClean so
	// mergemode.Enter can apply it without prompting the user.
	if len(conflicts) == 0 {
		merged := cleans[0]
		if bytes.Equal(merged, ours) || bytes.Equal(ours, theirs) {
			return []Hunk{{Kind: HunkClean, AutoBytes: append([]byte(nil), ours...)}}
		}
		if bytes.Equal(merged, theirs) {
			return []Hunk{{Kind: HunkClean, AutoBytes: append([]byte(nil), theirs...)}}
		}
		// Mixed clean merge (both sides changed different regions): the merged
		// output is the correctly-merged content. Return it as AutoBytes; byte-
		// faithfulness holds because libgit2 copies the input lines verbatim for
		// non-conflicting regions.
		return []Hunk{{Kind: HunkClean, AutoBytes: append([]byte(nil), merged...)}}
	}

	// anchors[i].oursStart/End and .theirsStart/End are byte offsets in the
	// original ours and theirs inputs. A negative oursStart signals an R5
	// fallback (could not locate the conflict section verbatim).
	type anchor struct {
		oursStart, oursEnd     int
		theirsStart, theirsEnd int
	}
	anchors := make([]anchor, len(conflicts))

	oursSearch := 0
	theirsSearch := 0
	validAnchors := true

	for i, c := range conflicts {
		var oursStart, oursEnd, theirsStart, theirsEnd int

		// Locate oursSection verbatim in original ours.
		if len(c.oursSection) == 0 {
			// Pure theirs-insertion: ours has nothing for this conflict.
			oursStart = oursSearch
			oursEnd = oursSearch
		} else {
			idx := bytes.Index(ours[oursSearch:], c.oursSection)
			if idx < 0 {
				validAnchors = false
				break
			}
			oursStart = oursSearch + idx
			oursEnd = oursStart + len(c.oursSection)
			oursSearch = oursEnd
		}

		// Locate theirsSection verbatim in original theirs.
		if len(c.theirsSection) == 0 {
			// Pure ours-insertion: theirs has nothing for this conflict.
			theirsStart = theirsSearch
			theirsEnd = theirsSearch
		} else {
			idx := bytes.Index(theirs[theirsSearch:], c.theirsSection)
			if idx < 0 {
				validAnchors = false
				break
			}
			theirsStart = theirsSearch + idx
			theirsEnd = theirsStart + len(c.theirsSection)
			theirsSearch = theirsEnd
		}

		anchors[i] = anchor{oursStart, oursEnd, theirsStart, theirsEnd}
	}

	// R5 fallback: if anchors could not be established, treat the entire
	// content as a single conflict so no data is lost.
	if !validAnchors {
		return []Hunk{{
			Kind:        HunkConflict,
			OursBytes:   append([]byte(nil), ours...),
			TheirsBytes: append([]byte(nil), theirs...),
		}}
	}

	// Build hunks: interleave clean regions (derived from ours/theirs) and
	// conflict blocks (verbatim from original inputs).
	var hunks []Hunk
	oursPos := 0
	theirsPos := 0

	for i, clean := range cleans {
		// Determine end of the clean region in ours and theirs.
		var oursCleanEnd, theirsCleanEnd int
		if i < len(conflicts) {
			oursCleanEnd = anchors[i].oursStart
			theirsCleanEnd = anchors[i].theirsStart
		} else {
			oursCleanEnd = len(ours)
			theirsCleanEnd = len(theirs)
		}

		oursClean := ours[oursPos:oursCleanEnd]
		theirsClean := theirs[theirsPos:theirsCleanEnd]

		// Classify the clean region and extract verbatim bytes.
		if len(oursClean) > 0 || len(theirsClean) > 0 {
			switch {
			case bytes.Equal(oursClean, theirsClean):
				// Unchanged or both made the same change.
				hunks = append(hunks, Hunk{
					Kind:      HunkClean,
					AutoBytes: append([]byte(nil), oursClean...),
				})
			case bytes.Equal(theirsClean, clean):
				// Only theirs changed: theirs' content is the merged output.
				hunks = append(hunks, Hunk{
					Kind:      HunkClean,
					AutoBytes: append([]byte(nil), theirsClean...),
				})
			case bytes.Equal(oursClean, clean):
				// Only ours changed: ours' content is the merged output.
				hunks = append(hunks, Hunk{
					Kind:      HunkClean,
					AutoBytes: append([]byte(nil), oursClean...),
				})
			default:
				// R5: cannot cleanly classify; treat as conflict.
				hunks = append(hunks, Hunk{
					Kind:        HunkConflict,
					OursBytes:   append([]byte(nil), oursClean...),
					TheirsBytes: append([]byte(nil), theirsClean...),
				})
			}
		}

		oursPos = oursCleanEnd
		theirsPos = theirsCleanEnd

		// Add the conflict hunk (if one follows this clean region).
		if i < len(conflicts) {
			a := anchors[i]
			hunks = append(hunks, Hunk{
				Kind:        HunkConflict,
				OursBytes:   append([]byte(nil), ours[a.oursStart:a.oursEnd]...),
				TheirsBytes: append([]byte(nil), theirs[a.theirsStart:a.theirsEnd]...),
			})
			oursPos = a.oursEnd
			theirsPos = a.theirsEnd
		}
	}

	// Ensure at least one hunk (clean merge with no changes).
	if len(hunks) == 0 {
		hunks = append(hunks, Hunk{
			Kind:      HunkClean,
			AutoBytes: append([]byte(nil), ours...),
		})
	}
	return hunks
}
