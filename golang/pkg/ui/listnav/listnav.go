// Package listnav is the shared cursor/viewport navigation seam for the
// UI's linear-list components (filetree, opentabs). Before this package
// existed, cursor±clamp arithmetic, a mouseScrollLines constant, and the
// click-row-to-index formula were each hand-rolled per component (F33) —
// this collapses them into one leaf type. A leaf per §10: it imports only
// pkg/ui/scroll (the tea.Mouse*Msg types it maps onto ints/bools stay the
// caller's problem, so it needs no bubbletea import), never a page or a
// sibling component.
package listnav

import "rune/pkg/ui/scroll"

// WheelLines is the number of rows a single mouse-wheel tick moves the
// cursor. Replaces the two formerly-duplicated mouseScrollLines constants
// (filetree/mouse.go, markdownedit/mouse.go — the latter scrolls a
// viewport, not a list cursor, but shares the same "how many rows per
// tick" constant).
const WheelLines = 3

// List is a cursor/viewport pair for navigating a linear list. The zero
// value (Cursor=0, Top=0) is ready to use.
type List struct {
	Cursor int
	Top    int
}

// Move shifts Cursor by delta, clamped to [0, total). total<=0 forces
// Cursor to 0 (an empty list has no valid cursor position).
func (l List) Move(delta, total int) List {
	if total <= 0 {
		l.Cursor = 0
		return l
	}
	l.Cursor += delta
	if l.Cursor < 0 {
		l.Cursor = 0
	}
	if l.Cursor >= total {
		l.Cursor = total - 1
	}
	return l
}

// First moves the cursor to the first item.
func (l List) First() List {
	l.Cursor = 0
	return l
}

// Last moves the cursor to the last item. total<=0 forces Cursor to 0.
func (l List) Last(total int) List {
	if total <= 0 {
		l.Cursor = 0
		return l
	}
	l.Cursor = total - 1
	return l
}

// Follow adjusts Top so Cursor stays within the visible window of viewRows
// rows over total items, per scroll.Follow (jump=0 — every existing caller
// wants minimal line-by-line scroll, not a jump-ahead).
func (l List) Follow(viewRows, total, margin int) List {
	l.Top = scroll.Follow(l.Cursor, l.Top, viewRows, total, margin, 0)
	return l
}

// Window returns the [start, end) index range visible for viewRows rows
// over total items, given the current Top. Empty (0, 0) when there is
// nothing to show.
func (l List) Window(viewRows, total int) (start, end int) {
	if viewRows <= 0 || total <= 0 {
		return 0, 0
	}
	start = l.Top
	if start < 0 {
		start = 0
	}
	if start > total {
		start = total
	}
	end = start + viewRows
	if end > total {
		end = total
	}
	return start, end
}

// Wheel moves the cursor by WheelLines rows (up=true moves toward index 0),
// clamped to [0, total) — the shared mouse-wheel-to-cursor mapping.
func (l List) Wheel(up bool, total int) List {
	delta := WheelLines
	if up {
		delta = -WheelLines
	}
	return l.Move(delta, total)
}

// ClickIndex maps a mouse click's absolute row (clickY) to a list index.
// offsetY is the component's absolute screen offset; headerRows is the
// number of non-item rows rendered before the first list item (e.g. a pane
// title or a "── Open ──" divider); top is the current scroll offset
// (List.Top). ok is false when the click lands on the header or beyond the
// last item.
func ClickIndex(clickY, offsetY, headerRows, top, total int) (index int, ok bool) {
	relY := clickY - offsetY
	if relY < headerRows {
		return 0, false
	}
	idx := top + (relY - headerRows)
	if idx < 0 || idx >= total {
		return 0, false
	}
	return idx, true
}
