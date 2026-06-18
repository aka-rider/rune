package scroll_test

import (
	"testing"

	"rune/pkg/ui/scroll"
)

func TestFollow(t *testing.T) {
	tests := []struct {
		name   string
		cursor int
		offset int
		size   int
		total  int
		margin int
		jump   int
		want   int
	}{
		// --- basic margin enforcement ---
		{
			name: "cursor well inside viewport, no scroll",
			// viewport [0,9], cursor=5, margin=2: 5 is between [2,7] — no change
			cursor: 5, offset: 0, size: 10, total: 20, margin: 2, jump: 0,
			want: 0,
		},
		{
			name: "cursor at top margin boundary, no scroll",
			// viewport [0,9], margin=2: top band end = offset+margin = 2; cursor=2 is at boundary, still OK
			cursor: 2, offset: 0, size: 10, total: 20, margin: 2, jump: 0,
			want: 0,
		},
		{
			name: "cursor breaches top margin",
			// viewport [5,14], cursor=6, margin=2: need cursor >= offset+margin=7 → offset moves to cursor-margin=4
			cursor: 6, offset: 5, size: 10, total: 20, margin: 2, jump: 0,
			want: 4,
		},
		{
			name: "cursor breaches bottom margin",
			// viewport [0,9], cursor=8, margin=2: bottom band start = offset+size-1-margin = 7; cursor=8 > 7 → offset = 8-(9-2) = 1
			cursor: 8, offset: 0, size: 10, total: 20, margin: 2, jump: 0,
			want: 1,
		},

		// --- jump on horizontal breach ---
		{
			name: "cursor breaches right margin with jump",
			// size=20, margin=4, jump=5; bottom threshold = offset+20-1-4 = 14
			// cursor=15 > 14 → offset = 15-(19-4)+5 = 15-15+5 = 5
			cursor: 15, offset: 0, size: 20, total: 100, margin: 4, jump: 5,
			want: 5,
		},
		{
			name: "cursor breaches left margin with jump",
			// offset=10, margin=4, jump=5; cursor=13 < 10+4=14 → offset = 13-4-5 = 4
			cursor: 13, offset: 10, size: 20, total: 100, margin: 4, jump: 5,
			want: 4,
		},

		// --- margin NOT enforced at content ends (clamping) ---
		{
			name: "cursor at row 0 — offset clamped to 0, margin not forced",
			// Even with margin=4 the offset cannot go negative
			cursor: 0, offset: 0, size: 10, total: 20, margin: 4, jump: 0,
			want: 0,
		},
		{
			name: "cursor at last row — offset clamped to total-size",
			// total=20, size=10, cursor=19; bottom threshold = offset+size-1-margin
			// offset moves to keep cursor visible but clamped to max 10
			cursor: 19, offset: 10, size: 10, total: 20, margin: 4, jump: 0,
			want: 10,
		},
		{
			name: "content shorter than viewport — offset stays 0",
			// total=5, size=10 → maxOff=0; cursor anywhere still gets offset=0
			cursor: 3, offset: 0, size: 10, total: 5, margin: 2, jump: 0,
			want: 0,
		},

		// --- tiny pane: margin degrades without panic ---
		{
			name: "size=1 — margin degrades to 0",
			cursor: 5, offset: 4, size: 1, total: 20, margin: 4, jump: 0,
			want: 5,
		},
		{
			name: "size=2, margin=1 — 2*margin==size, no degradation, cursor at boundary scrolls",
			cursor: 5, offset: 4, size: 2, total: 20, margin: 1, jump: 0,
			want: 5,
		},
		{
			name: "size=3, margin=2 — degrades to 1",
			// 2*2=4 >= 3, so margin = (3-1)/2 = 1
			cursor: 5, offset: 3, size: 3, total: 20, margin: 2, jump: 0,
			want: 4,
		},

		// --- size <= 0 ---
		{
			name:   "size=0 returns 0",
			cursor: 10, offset: 5, size: 0, total: 20, margin: 4, jump: 0,
			want:   0,
		},
		{
			name:   "size negative returns 0",
			cursor: 10, offset: 5, size: -1, total: 20, margin: 4, jump: 0,
			want:   0,
		},

		// --- downward/upward moves stay within margin mid-document ---
		{
			name: "scrolling down keeps cursor inside bottom margin",
			// viewport [3,12], cursor=10, margin=2: bottom threshold=3+10-1-2=10; cursor==10 (not >) — no scroll
			cursor: 10, offset: 3, size: 10, total: 30, margin: 2, jump: 0,
			want: 3,
		},
		{
			name: "one step past bottom margin triggers scroll",
			// viewport [3,12], cursor=11 > 10 → offset = 11-7 = 4
			cursor: 11, offset: 3, size: 10, total: 30, margin: 2, jump: 0,
			want: 4,
		},
		{
			name: "scrolling up keeps cursor inside top margin",
			cursor: 5, offset: 3, size: 10, total: 30, margin: 2, jump: 0,
			// top threshold = offset+margin = 5; cursor==5 (not <) — no scroll
			want: 3,
		},
		{
			name: "one step past top margin triggers scroll",
			// cursor=4 < 3+2=5 → offset = 4-2 = 2
			cursor: 4, offset: 3, size: 10, total: 30, margin: 2, jump: 0,
			want: 2,
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := scroll.Follow(tc.cursor, tc.offset, tc.size, tc.total, tc.margin, tc.jump)
			if got != tc.want {
				t.Errorf("Follow(cursor=%d, offset=%d, size=%d, total=%d, margin=%d, jump=%d) = %d, want %d",
					tc.cursor, tc.offset, tc.size, tc.total, tc.margin, tc.jump, got, tc.want)
			}
		})
	}
}
