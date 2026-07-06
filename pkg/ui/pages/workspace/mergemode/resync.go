package mergemode

import (
	"strings"

	"rune/pkg/ui/components/markdownedit"
)

// Resync re-derives blocks/active from the editor's current buffer. Called by
// the workspace after undo/redo so the merge view reflects a journal jump that
// did not go through HandleKey (active=false ⇒ the caller exits merge mode).
//
// Slot-ordered AND content-verifying (critic R1): resolved regions and clean
// AutoBytes can themselves contain a literal "<<<<<<<" / "=======" / ">>>>>>>"
// line (a doc that quotes git markers, or one that already had real conflict
// markers). A naive "Nth <<<<<<< anchor is the Nth conflict" scan would then
// treat a spurious anchor as a block start and shift every later mapping →
// collapse a block to the wrong immutable side (rung-1). Instead, Resync walks
// the immutable conflict list in order and, for each conflict k, scans FORWARD
// from the current search cursor for a byte-exact match of the FULL framed
// block ("<<<<<<< ours\n"+ours[k]+"\n=======\n"+theirs[k]+"\n>>>>>>> theirs\n")
// — never a bare anchor or a bare "=======" line. A spurious "<<<<<<<" inside
// quoted content can never satisfy this exact match (it would need the SAME
// verbatim ours[k]/theirs[k] bytes framed the same way), so it is skipped as
// ordinary content, not mistaken for a block boundary.
//
// If the full framed block is not found, the conflict has been resolved to one
// side (accept always collapses a block to exactly ours[k] or theirs[k] bytes
// — the buffer is never free-edited during merge, §3) — Resync locates
// whichever of the two occurs first from the search cursor so the next
// conflict's search continues from the right position.
func Resync(st State, ed markdownedit.Model) State {
	if len(st.conflicts) == 0 {
		st.active = false
		st.cur = -1
		return st
	}

	content := ed.Content()
	newBlocks := make([]block, len(st.conflicts))
	searchFrom := 0

	for k, c := range st.conflicts {
		framed := frameBlock(c.ours, c.theirs)
		if idx := indexFrom(content, framed, searchFrom); idx >= 0 {
			newBlocks[k] = block{start: idx, end: idx + len(framed), resolved: false}
			searchFrom = idx + len(framed)
			continue
		}

		oursIdx := indexFrom(content, c.ours, searchFrom)
		theirsIdx := indexFrom(content, c.theirs, searchFrom)
		switch {
		case oursIdx >= 0 && (theirsIdx < 0 || oursIdx <= theirsIdx):
			newBlocks[k] = block{start: oursIdx, end: oursIdx + len(c.ours), resolved: true}
			searchFrom = oursIdx + len(c.ours)
		case theirsIdx >= 0:
			newBlocks[k] = block{start: theirsIdx, end: theirsIdx + len(c.theirs), resolved: true}
			searchFrom = theirsIdx + len(c.theirs)
		default:
			// Neither the framed block nor either resolved form is found — an
			// inconsistent buffer the merge invariants should prevent. Degrade
			// safely (never panic, §1.3): mark resolved with no advance rather
			// than misclassify as an open, un-navigable conflict.
			newBlocks[k] = block{start: searchFrom, end: searchFrom, resolved: true}
		}
	}

	st.blocks = newBlocks
	st.active = firstUnresolved(newBlocks) >= 0
	if st.active {
		if st.cur < 0 || st.cur >= len(newBlocks) || newBlocks[st.cur].resolved {
			st.cur = firstUnresolved(newBlocks)
		}
	} else {
		st.cur = -1
	}
	st = st.refreshView(ed)
	return st
}

// indexFrom returns the byte offset of the first occurrence of needle in
// content at or after from, or -1 if absent or from is out of range.
func indexFrom(content string, needle []byte, from int) int {
	if from < 0 || from > len(content) {
		return -1
	}
	idx := strings.Index(content[from:], string(needle))
	if idx < 0 {
		return -1
	}
	return idx + from
}
