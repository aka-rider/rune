package buffer

import (
	"errors"
	"sort"
	"strings"
	"unicode/utf8"

	"rune/pkg/editor/coords"
)

type Edit struct {
	Start  int
	End    int
	Insert string
}

type AppliedEdit struct {
	Start   int
	End     int
	Deleted string
	Insert  string
}

type Buffer struct {
	content    string
	lineStarts []int
	version    uint64
}

func New(content string) Buffer {
	return Buffer{
		content:    content,
		lineStarts: computeLineStarts(content),
		version:    1,
	}
}

func FromBytes(content []byte) (Buffer, error) {
	if !utf8.Valid(content) {
		return Buffer{}, errors.New("invalid UTF-8 sequence")
	}
	return New(string(content)), nil
}

func (b Buffer) Empty() bool {
	return b.Len() == 0
}

func (b Buffer) Len() int {
	return len(b.content)
}

func (b Buffer) Version() uint64 {
	return b.version
}

func (b Buffer) Content() string {
	return b.content
}

func (b Buffer) Slice(start, end int) string {
	return b.content[start:end]
}

func (b Buffer) Byte(offset int) byte {
	return b.content[offset]
}

func (b Buffer) RuneAt(offset int) (rune, int) {
	return utf8.DecodeRuneInString(b.content[offset:])
}

func (b Buffer) Insert(offset int, text string) Buffer {
	return b.Replace(offset, offset, text)
}

func (b Buffer) Delete(start, end int) Buffer {
	return b.Replace(start, end, "")
}

func (b Buffer) Replace(start, end int, text string) Buffer {
	newB, _, _ := b.ApplyEdits([]Edit{{Start: start, End: end, Insert: text}})
	return newB
}

func IsSortedDescendingNonOverlapping(edits []Edit) bool {
	for i := 0; i < len(edits)-1; i++ {
		if edits[i].Start < edits[i+1].End {
			return false
		}
	}
	return true
}

func CloneAndSortEditsDescending(edits []Edit) []Edit {
	cloned := make([]Edit, len(edits))
	copy(cloned, edits)
	sort.Slice(cloned, func(i, j int) bool {
		if cloned[i].Start == cloned[j].Start {
			return cloned[i].End > cloned[j].End
		}
		return cloned[i].Start > cloned[j].Start
	})
	return cloned
}

func (b Buffer) ApplyEdits(edits []Edit) (Buffer, []AppliedEdit, error) {
	if len(edits) == 0 {
		return b, nil, nil
	}

	if !IsSortedDescendingNonOverlapping(edits) {
		return b, nil, errors.New("edits must be non-overlapping and sorted descending")
	}

	for _, e := range edits {
		if e.Start < 0 || e.End > len(b.content) || e.Start > e.End {
			return b, nil, errors.New("edit out of bounds")
		}
		if e.Start > 0 && e.Start < len(b.content) && !utf8.RuneStart(b.content[e.Start]) {
			return b, nil, errors.New("edit start splits a rune")
		}
		if e.End > 0 && e.End < len(b.content) && !utf8.RuneStart(b.content[e.End]) {
			return b, nil, errors.New("edit end splits a rune")
		}
		if !utf8.ValidString(e.Insert) {
			return b, nil, errors.New("edit insert is invalid utf-8")
		}
	}

	var netChange int
	for _, e := range edits {
		netChange += len(e.Insert) - (e.End - e.Start)
	}

	var bldr strings.Builder
	bldr.Grow(len(b.content) + netChange)

	applied := make([]AppliedEdit, len(edits))
	shifts := make([]int, len(edits))
	currentShift := 0

	for i := len(edits) - 1; i >= 0; i-- {
		e := edits[i]
		shifts[i] = currentShift
		currentShift += len(e.Insert) - (e.End - e.Start)
	}

	lastEnd := 0
	for i := len(edits) - 1; i >= 0; i-- {
		e := edits[i]
		bldr.WriteString(b.content[lastEnd:e.Start])
		bldr.WriteString(e.Insert)
		lastEnd = e.End

		applied[i] = AppliedEdit{
			Start:   e.Start + shifts[i],
			End:     e.Start + shifts[i] + len(e.Insert),
			Deleted: b.content[e.Start:e.End],
			Insert:  e.Insert,
		}
	}
	bldr.WriteString(b.content[lastEnd:])

	newContent := bldr.String()
	newLineStarts := b.updateLineStarts(edits)

	return Buffer{
		content:    newContent,
		lineStarts: newLineStarts,
		version:    b.version + 1,
	}, applied, nil
}

func (b Buffer) LineCount() int {
	return len(b.getLineStarts())
}

func (b Buffer) LineStart(n int) int {
	starts := b.getLineStarts()
	if n < 0 || n >= len(starts) {
		return 0
	}
	return starts[n]
}

func (b Buffer) LineEnd(n int) int {
	starts := b.getLineStarts()
	if n < 0 || n >= len(starts) {
		return 0
	}
	if n == len(starts)-1 {
		return len(b.content)
	}
	return starts[n+1] - 1
}

func (b Buffer) Line(n int) string {
	start := b.LineStart(n)
	end := b.LineEnd(n)
	if start <= end && start >= 0 && end <= len(b.content) {
		return b.content[start:end]
	}
	return ""
}

func (b Buffer) OffsetToLineCol(offset int) coords.BufferPoint {
	if offset < 0 {
		return coords.BufferPoint{Line: 0, Col: 0}
	}
	if offset > len(b.content) {
		offset = len(b.content)
	}
	line := findLine(b.getLineStarts(), offset)
	col := offset - b.getLineStarts()[line]
	return coords.BufferPoint{Line: line, Col: col}
}

func (b Buffer) LineColToOffset(bp coords.BufferPoint) int {
	starts := b.getLineStarts()
	if bp.Line < 0 {
		return 0
	}
	if bp.Line >= len(starts) {
		return len(b.content)
	}
	offset := starts[bp.Line] + bp.Col
	end := b.LineEnd(bp.Line)
	if bp.Line == len(starts)-1 {
		end = len(b.content)
	}
	if offset > end {
		if bp.Line < len(starts)-1 {
			return end
		}
		return end
	}
	return offset
}
