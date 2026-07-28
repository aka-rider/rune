package buffer

import "sort"

// ReplayForward applies a sequence of edit batches to content and returns the
// resulting string. Each inner slice is one batch (as stored by AppendEdit),
// applied in the order they appear in batches. Within a batch, edits are
// sorted ascending by Start so that multi-cursor baked-in offsets align
// correctly with the running string displacement.
//
// Used by docstate.RecoverDocument to replay events from a snapshot forward
// to current_seq without importing the UI-layer markdownedit package.
func ReplayForward(content string, batches [][]AppliedEdit) string {
	for _, batch := range batches {
		content = replayOneBatch(content, batch)
	}
	return content
}

func replayOneBatch(s string, batch []AppliedEdit) string {
	sorted := make([]AppliedEdit, len(batch))
	copy(sorted, batch)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].Start < sorted[j].Start
	})
	for _, e := range sorted {
		if e.Start < 0 || e.Start > len(s) {
			continue
		}
		skip := len(e.Deleted)
		tail := e.Start + skip
		if tail > len(s) {
			tail = len(s)
		}
		s = s[:e.Start] + e.Insert + s[tail:]
	}
	return s
}
