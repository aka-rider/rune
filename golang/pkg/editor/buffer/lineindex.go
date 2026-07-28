package buffer

import "sort"

func computeLineStarts(content string) []int {
	starts := []int{0}
	for i := 0; i < len(content); i++ {
		if content[i] == '\n' {
			starts = append(starts, i+1)
		}
	}
	return starts
}

func (b Buffer) getLineStarts() []int {
	if b.lineStarts == nil {
		return []int{0}
	}
	return b.lineStarts
}

func (b Buffer) updateLineStarts(edits []Edit) []int {
	lineStarts := make([]int, len(b.getLineStarts()))
	copy(lineStarts, b.getLineStarts())

	for _, e := range edits {
		startLine := findLine(lineStarts, e.Start)
		endLine := findLine(lineStarts, e.End)

		addedStarts := computeAddedStarts(e.Start, e.Insert)
		delta := len(e.Insert) - (e.End - e.Start)

		for i := endLine + 1; i < len(lineStarts); i++ {
			lineStarts[i] += delta
		}

		head := lineStarts[:startLine+1]
		tail := lineStarts[endLine+1:]

		newLen := len(head) + len(addedStarts) + len(tail)
		nextStarts := make([]int, 0, newLen)
		nextStarts = append(nextStarts, head...)
		nextStarts = append(nextStarts, addedStarts...)
		nextStarts = append(nextStarts, tail...)

		lineStarts = nextStarts
	}
	return lineStarts
}

func computeAddedStarts(baseOffset int, text string) []int {
	var starts []int
	for i := 0; i < len(text); i++ {
		if text[i] == '\n' {
			starts = append(starts, baseOffset+i+1)
		}
	}
	return starts
}

func findLine(starts []int, offset int) int {
	if len(starts) == 0 {
		return 0
	}
	i := sort.Search(len(starts), func(i int) bool {
		return starts[i] > offset
	})
	return i - 1
}
