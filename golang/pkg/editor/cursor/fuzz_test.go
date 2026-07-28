package cursor

import (
	"math/rand"
	"testing"
	"time"

	"rune/pkg/editor/buffer"
)

func TestCursorSetBoundsFuzzer(t *testing.T) {
	// P1 & P8: AdjustAfterBatchEdits produces valid cursors inside [0, newBufLen]
	rng := rand.New(rand.NewSource(time.Now().UnixNano()))

	for i := 0; i < 1000; i++ {
		// Generate random initial buffer len
		bufLen := rng.Intn(1000) + 1

		// Generate random cursors
		numCursors := rng.Intn(10) + 1
		cursors := make([]Cursor, numCursors)
		for j := 0; j < numCursors; j++ {
			c := Cursor{
				Position: rng.Intn(bufLen + 1),
				Anchor:   rng.Intn(bufLen + 1),
				ID:       j + 1,
			}
			cursors[j] = c
		}
		cs := NewCursorSetFrom(cursors)

		// Generate random valid non-overlapping batch edits strictly within bounds
		// Since edits are applied conceptually descending or ascending, let's make valid offsets.
		numEdits := rng.Intn(5) + 1
		var rawEdits []buffer.Edit

		boundaries := []int{0, bufLen}
		for j := 0; j < numEdits*2; j++ {
			boundaries = append(boundaries, rng.Intn(bufLen+1))
		}

		for i := 0; i < len(boundaries); i++ {
			for j := i + 1; j < len(boundaries); j++ {
				if boundaries[i] > boundaries[j] {
					boundaries[i], boundaries[j] = boundaries[j], boundaries[i]
				}
			}
		}

		for j := 0; j < numEdits; j++ {
			start := boundaries[j*2+1]
			end := boundaries[j*2+2]
			if start > end {
				start, end = end, start
			}

			insertLen := rng.Intn(50)
			rawEdits = append(rawEdits, buffer.Edit{
				Start:  start,
				End:    end,
				Insert: string(make([]byte, insertLen)),
			})
		}

		for i := 0; i < len(rawEdits); i++ {
			for j := i + 1; j < len(rawEdits); j++ {
				if rawEdits[i].Start < rawEdits[j].Start {
					rawEdits[i], rawEdits[j] = rawEdits[j], rawEdits[i]
				}
			}
		}

		buf := buffer.New(string(make([]byte, bufLen)))
		newBuf, edits, err := buf.ApplyEdits(rawEdits)
		if err != nil {
			continue // Should not happen with valid bounds
		}
		currentBufLen := newBuf.Len()

		adjusted := cs.AdjustAfterBatchEdits(edits)

		for _, c := range adjusted.All() {
			if c.Position < 0 || c.Position > currentBufLen {
				t.Fatalf("AdjustAfterBatchEdits P1 violation: position %d not in [0, %d]. Initial bufLen=%d, edits=%+v", c.Position, currentBufLen, bufLen, edits)
			}
			if c.Anchor < 0 || c.Anchor > currentBufLen {
				t.Fatalf("AdjustAfterBatchEdits P1 violation: anchor %d not in [0, %d]. Initial bufLen=%d, edits=%+v", c.Anchor, currentBufLen, bufLen, edits)
			}
		}
	}
}
