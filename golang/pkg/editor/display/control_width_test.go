package display

import "testing"

// TestControlAwareWidth locks the shared width rule that the wrap/coordinate
// layer and the cell renderer must both obey. A disagreement here strands the
// cursor with no rendered cell on a narrow viewport (fuzz invariant R1, seed
// FuzzSessionWithFile/943e32fda4c8d59c).
func TestControlAwareWidth(t *testing.T) {
	cases := []struct {
		name string
		r    rune
		want int
	}{
		{"newline occupies no column (no cell)", '\n', 0},
		{"carriage return occupies no column (no cell)", '\r', 0},
		{"NUL clamps to 1 (cell renderer draws a cell)", '\x00', 1},
		{"ACK control clamps to 1", '\x06', 1},
		{"FS control clamps to 1", '\x1c', 1},
		{"combining acute clamps to 1", '́', 1},
		{"ASCII letter is 1", 'a', 1},
		{"space is 1", ' ', 1},
		{"wide CJK is 2", '世', 2},
	}
	for _, c := range cases {
		if got := ControlAwareWidth(c.r); got != c.want {
			t.Errorf("%s: ControlAwareWidth(%q) = %d, want %d", c.name, c.r, got, c.want)
		}
	}
}
