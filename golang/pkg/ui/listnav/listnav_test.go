package listnav_test

import (
	"testing"

	"rune/pkg/ui/listnav"
)

// TestMove_ClampsToRange: for every total and every starting cursor, Move by
// any delta lands in [0, total) (or stays 0 when total<=0).
func TestMove_ClampsToRange(t *testing.T) {
	for total := -1; total <= 12; total++ {
		for start := -3; start <= 15; start++ {
			for delta := -20; delta <= 20; delta += 5 {
				l := listnav.List{Cursor: start}
				got := l.Move(delta, total)
				if total <= 0 {
					if got.Cursor != 0 {
						t.Fatalf("Move(delta=%d,total=%d) from cursor=%d = %d, want 0", delta, total, start, got.Cursor)
					}
					continue
				}
				if got.Cursor < 0 || got.Cursor >= total {
					t.Fatalf("Move(delta=%d,total=%d) from cursor=%d = %d, out of [0,%d)", delta, total, start, got.Cursor, total)
				}
			}
		}
	}
}

// TestFirstLast: First always lands on 0; Last always lands on total-1 (or 0
// when total<=0).
func TestFirstLast(t *testing.T) {
	for total := -1; total <= 10; total++ {
		l := listnav.List{Cursor: 7, Top: 3}
		if got := l.First(); got.Cursor != 0 {
			t.Fatalf("First() = %d, want 0", got.Cursor)
		}
		got := l.Last(total)
		want := total - 1
		if total <= 0 {
			want = 0
		}
		if got.Cursor != want {
			t.Fatalf("Last(%d) = %d, want %d", total, got.Cursor, want)
		}
	}
}

// TestFollow_CursorAlwaysInWindow is the core property: after Follow, the
// cursor must land inside the [start,end) window Window() reports for the
// SAME viewRows/total — the whole point of Follow is "keep the cursor
// visible". Swept over a grid of total/viewRows/margin/cursor combinations,
// simulating incremental cursor moves (so Top carries state across calls,
// as every real caller does).
func TestFollow_CursorAlwaysInWindow(t *testing.T) {
	for total := 0; total <= 30; total += 3 {
		for viewRows := 0; viewRows <= 10; viewRows++ {
			for margin := 0; margin <= 5; margin++ {
				var l listnav.List
				// Walk the cursor from 0 to total-1 and back down, Follow-ing
				// after every step — the realistic key-repeat pattern.
				positions := make([]int, 0, total*2)
				for c := 0; c < total; c++ {
					positions = append(positions, c)
				}
				for c := total - 1; c >= 0; c-- {
					positions = append(positions, c)
				}
				for _, c := range positions {
					l.Cursor = c
					l = l.Follow(viewRows, total, margin)

					if viewRows <= 0 || total <= 0 {
						continue // nothing to show — no window to be inside
					}
					start, end := l.Window(viewRows, total)
					if l.Cursor < start || l.Cursor >= end {
						t.Fatalf("total=%d viewRows=%d margin=%d cursor=%d: window=[%d,%d) does not contain cursor",
							total, viewRows, margin, l.Cursor, start, end)
					}
				}
			}
		}
	}
}

// TestFollow_TopClamped: Top must always sit in [0, max(0,total-viewRows)] —
// scroll.Follow's own contract, re-asserted here since listnav is the public
// seam callers rely on.
func TestFollow_TopClamped(t *testing.T) {
	for total := 0; total <= 25; total += 5 {
		for viewRows := 0; viewRows <= 8; viewRows += 2 {
			for margin := 0; margin <= 4; margin++ {
				for cursor := -2; cursor <= total+2; cursor++ {
					l := listnav.List{Cursor: cursor}
					l = l.Follow(viewRows, total, margin)

					maxTop := total - viewRows
					if maxTop < 0 {
						maxTop = 0
					}
					if l.Top < 0 || l.Top > maxTop {
						t.Fatalf("total=%d viewRows=%d margin=%d cursor=%d: Top=%d out of [0,%d]",
							total, viewRows, margin, cursor, l.Top, maxTop)
					}
				}
			}
		}
	}
}

// TestWindow_Bounds: Window's [start,end) always sits inside [0,total], and
// end-start never exceeds viewRows.
func TestWindow_Bounds(t *testing.T) {
	for total := -1; total <= 20; total++ {
		for viewRows := -1; viewRows <= 10; viewRows++ {
			for top := -3; top <= 20; top++ {
				l := listnav.List{Top: top}
				start, end := l.Window(viewRows, total)
				if start < 0 || end < start {
					t.Fatalf("viewRows=%d total=%d top=%d: invalid window [%d,%d)", viewRows, total, top, start, end)
				}
				if total > 0 && end > total {
					t.Fatalf("viewRows=%d total=%d top=%d: window end %d exceeds total", viewRows, total, top, end)
				}
				if end-start > max(viewRows, 0) {
					t.Fatalf("viewRows=%d total=%d top=%d: window size %d exceeds viewRows", viewRows, total, top, end-start)
				}
			}
		}
	}
}

// TestWheel_ClampsAndDirects: Wheel moves WheelLines rows in the requested
// direction and clamps to [0,total).
func TestWheel_ClampsAndDirects(t *testing.T) {
	l := listnav.List{Cursor: 5}
	down := l.Wheel(false, 20)
	if down.Cursor != 5+listnav.WheelLines {
		t.Fatalf("Wheel(down) = %d, want %d", down.Cursor, 5+listnav.WheelLines)
	}
	up := l.Wheel(true, 20)
	if up.Cursor != 5-listnav.WheelLines {
		t.Fatalf("Wheel(up) = %d, want %d", up.Cursor, 5-listnav.WheelLines)
	}

	atTop := listnav.List{Cursor: 1}
	if got := atTop.Wheel(true, 20); got.Cursor != 0 {
		t.Fatalf("Wheel(up) near top = %d, want clamped to 0", got.Cursor)
	}
	atBottom := listnav.List{Cursor: 18}
	if got := atBottom.Wheel(false, 20); got.Cursor != 19 {
		t.Fatalf("Wheel(down) near bottom = %d, want clamped to 19", got.Cursor)
	}
}

// TestClickIndex mirrors filetree's pre-listnav click math (headerRows=1,
// title row or above ignored) and generalizes it: a click within the header
// or past the last item must report ok=false.
func TestClickIndex(t *testing.T) {
	cases := []struct {
		name                        string
		clickY, offsetY, headerRows int
		top, total                  int
		wantIdx                     int
		wantOK                      bool
	}{
		{"header row ignored", 1, 1, 1, 0, 10, 0, false},
		{"above header ignored", 0, 1, 1, 0, 10, 0, false},
		{"first visible row", 2, 1, 1, 0, 10, 0, true},
		{"second visible row", 3, 1, 1, 0, 10, 1, true},
		{"scrolled window offsets index", 2, 1, 1, 5, 10, 5, true},
		{"past last item ignored", 20, 1, 1, 0, 3, 0, false},
		{"zero header rows", 1, 1, 0, 0, 10, 0, true},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			idx, ok := listnav.ClickIndex(c.clickY, c.offsetY, c.headerRows, c.top, c.total)
			if ok != c.wantOK {
				t.Fatalf("ok = %v, want %v", ok, c.wantOK)
			}
			if ok && idx != c.wantIdx {
				t.Fatalf("idx = %d, want %d", idx, c.wantIdx)
			}
		})
	}
}
