package coords

import (
	"testing"
)

func TestCoordinateTypes(t *testing.T) {
	// Buffer Space
	var bo BufferOffset = 10
	bp := BufferPoint{Line: 1, Col: 5}

	if bo != 10 {
		t.Errorf("BufferOffset expected 10, got %v", bo)
	}
	if bp.Line != 1 || bp.Col != 5 {
		t.Errorf("BufferPoint expected {1, 5}, got %v", bp)
	}

	// Syntax Space
	var so SyntaxOffset = 20
	sp := SyntaxPoint{Line: 2, Col: 10}

	if so != 20 {
		t.Errorf("SyntaxOffset expected 20, got %v", so)
	}
	if sp.Line != 2 || sp.Col != 10 {
		t.Errorf("SyntaxPoint expected {2, 10}, got %v", sp)
	}

	// Wrap Space
	var wr WrapRow = 30
	wp := WrapPoint{Row: 3, Col: 15}

	if wr != 30 {
		t.Errorf("WrapRow expected 30, got %v", wr)
	}
	if wp.Row != 3 || wp.Col != 15 {
		t.Errorf("WrapPoint expected {3, 15}, got %v", wp)
	}

	// Display Space
	var dr DisplayRow = 40
	dp := DisplayPoint{Row: 4, Col: 20}

	if dr != 40 {
		t.Errorf("DisplayRow expected 40, got %v", dr)
	}
	if dp.Row != 4 || dp.Col != 20 {
		t.Errorf("DisplayPoint expected {4, 20}, got %v", dp)
	}
}
