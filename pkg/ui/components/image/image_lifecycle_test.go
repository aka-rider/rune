package image

import (
	"errors"
	"testing"

	"rune/pkg/imagekit"
	"rune/pkg/terminal"
)

// TestStaleGenDropped locks in the spawn-generation guard (M2): a same-path
// async result stamped with an OLDER Gen than the live instance's must be
// dropped, even though Path matches — the scenario a despawn/respawn (mtime
// replacement, §3 discoverNewImages) produces when an in-flight decode from
// the despawned instance lands after the new instance already spawned.
func TestStaleGenDropped(t *testing.T) {
	m := New("a.png", "/abs/a.png", 1, 0, terminal.TermCaps{}, imagekit.CellSize{}, 80, 24, nil)
	staleGen := m.gen // capture, then simulate a respawn bumping to a fresh gen
	m2 := New("a.png", "/abs/a.png", 1, 0, terminal.TermCaps{}, imagekit.CellSize{}, 80, 24, nil)
	if m2.gen == staleGen {
		t.Fatalf("respawned instance must get a distinct gen")
	}

	updated, cmd := m2.Update(UpdateMsg{
		Path: "a.png",
		Gen:  staleGen, // the OLD instance's gen — stale relative to m2
		inner: decodedMsg{
			path: "a.png", cols: 6, rows: 4, pxW: 48, pxH: 32,
		},
	})
	if updated.state != PendingDecode || updated.Cols() != 0 {
		t.Fatalf("stale-gen decodedMsg must be dropped, got state=%v cols=%d", updated.state, updated.Cols())
	}
	if cmd != nil {
		t.Fatalf("stale-gen result must not produce a Cmd")
	}
}

// TestWrongStateDropped locks in the state guard: a decodedMsg arriving while
// already past PendingDecode (e.g. a duplicate/late result) must not
// re-apply — only the first decode result for a given spawn may advance the
// state.
func TestWrongStateDropped(t *testing.T) {
	m := New("a.png", "/abs/a.png", 1, 0, terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty}, imagekit.CellSize{}, 80, 24, nil)
	m, _ = m.Update(UpdateMsg{Path: "a.png", Gen: m.gen, inner: decodedMsg{
		path: "a.png", cols: 6, rows: 4, pxW: 48, pxH: 32,
	}})
	if m.state != PendingTransmit {
		t.Fatalf("setup: expected PendingTransmit, got %v", m.state)
	}

	// A second, duplicate decodedMsg arrives (e.g. a stale reorder within the
	// same gen) — it must not reset dimensions or state.
	updated, cmd := m.Update(UpdateMsg{Path: "a.png", Gen: m.gen, inner: decodedMsg{
		path: "a.png", cols: 99, rows: 99, pxW: 999, pxH: 999,
	}})
	if updated.state != PendingTransmit || updated.Cols() != 6 || updated.rows != 4 {
		t.Fatalf("decodedMsg while not PendingDecode must be dropped, got state=%v cols=%d rows=%d",
			updated.state, updated.Cols(), updated.rows)
	}
	if cmd != nil {
		t.Fatalf("a dropped illegal-state message must not produce a Cmd")
	}
}

// TestErrorMsgTransitionsToFailed locks in the M2 rebuild of Failed: an
// ErrorMsg (Gen-guarded like UpdateMsg) drives the image to Failed with the
// error recorded, reachable from Err().
func TestErrorMsgTransitionsToFailed(t *testing.T) {
	m := New("a.png", "/abs/a.png", 1, 0, terminal.TermCaps{}, imagekit.CellSize{}, 80, 24, nil)
	wantErr := errors.New("read image: boom")

	m, cmd := m.Update(ErrorMsg{Path: "a.png", Gen: m.gen, Err: wantErr})
	if m.State() != Failed {
		t.Fatalf("expected Failed, got %v", m.State())
	}
	if !errors.Is(m.Err(), wantErr) {
		t.Fatalf("Err() = %v, want %v", m.Err(), wantErr)
	}
	if cmd != nil {
		t.Fatalf("entering Failed must not schedule a Cmd, got one")
	}

	// A stale-gen ErrorMsg (respawn happened in between) must not apply.
	m2 := New("a.png", "/abs/a.png", 1, 0, terminal.TermCaps{}, imagekit.CellSize{}, 80, 24, nil)
	updated, _ := m2.Update(ErrorMsg{Path: "a.png", Gen: m.gen /* stale, belongs to the first m */, Err: wantErr})
	if updated.State() == Failed {
		t.Fatalf("stale-gen ErrorMsg must not transition a fresh instance to Failed")
	}
}

// TestFailedDropsEverything locks in Failed as a sink state: once Failed, no
// further lifecycle message (decoded/transmitted/encoded/frameTick) may move
// it anywhere else, matching Height()==1/IsLive()==false staying stable.
func TestFailedDropsEverything(t *testing.T) {
	m := New("a.png", "/abs/a.png", 1, 0, terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty}, imagekit.CellSize{}, 80, 24, nil)
	m, _ = m.Update(ErrorMsg{Path: "a.png", Gen: m.gen, Err: errors.New("boom")})
	if m.State() != Failed {
		t.Fatalf("setup: expected Failed, got %v", m.State())
	}

	msgs := []struct {
		name string
		msg  UpdateMsg
	}{
		{"decodedMsg", UpdateMsg{Path: "a.png", Gen: m.gen, inner: decodedMsg{path: "a.png", cols: 6, rows: 4, pxW: 48, pxH: 32}}},
		{"transmittedMsg", UpdateMsg{Path: "a.png", Gen: m.gen, inner: transmittedMsg{path: "a.png"}}},
		{"encodedMsg", UpdateMsg{Path: "a.png", Gen: m.gen, inner: encodedMsg{path: "a.png", slices: []string{"x"}}}},
		{"frameTickMsg", UpdateMsg{Path: "a.png", Gen: m.gen, inner: frameTickMsg{path: "a.png", gen: 0, next: 1}}},
	}
	for _, c := range msgs {
		updated, cmd := m.Update(c.msg)
		if updated.State() != Failed {
			t.Fatalf("%s: Failed must be sticky, got state=%v", c.name, updated.State())
		}
		if cmd != nil {
			t.Fatalf("%s: Failed must never produce a Cmd", c.name)
		}
		if updated.Height() != 1 || updated.IsLive() {
			t.Fatalf("%s: Failed must keep Height()==1 and IsLive()==false", c.name)
		}
	}
}
