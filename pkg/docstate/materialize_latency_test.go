package docstate

import (
	"testing"
	"time"

	"rune/pkg/editor/buffer"
	"rune/pkg/vfs"
)

// TestMaterialize_NoTxAcrossDiskIO is the plan's latency guard: with a
// sleeping FS hook inside Materialize's disk I/O, a concurrent-goroutine
// AppendEdit must complete WITHOUT waiting for the sleep. Store.perm is
// SetMaxOpenConns(1) — a single physical connection — so if Materialize held
// a DB tx open across that slow disk call, AppendEdit's own tx.Begin() on
// the SAME connection would necessarily block until Materialize's tx ended,
// proving the contract violated. A fast AppendEdit is only possible if no
// tx is open while the hook sleeps.
func TestMaterialize_NoTxAcrossDiskIO(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}
	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	expect, _, err := s.SavedObs(loaded.DocID)
	if err != nil {
		t.Fatalf("SavedObs: %v", err)
	}

	const sleepDur = 300 * time.Millisecond
	hookStarted := make(chan struct{})
	hook := &hookFS{FS: mem, beforeExchange: func(real vfs.FS) {
		close(hookStarted)
		time.Sleep(sleepDur)
	}}
	s.UseFS(hook)

	matDone := make(chan error, 1)
	go func() {
		_, err := s.Materialize(loaded.DocID, path, "new content", expect.ID, 0, false)
		matDone <- err
	}()

	<-hookStarted // Materialize is now inside the slow disk call.

	appendDone := make(chan error, 1)
	go func() {
		_, err := s.AppendEdit(loaded.DocID, []buffer.AppliedEdit{{Insert: "x"}}, nil, nil)
		appendDone <- err
	}()

	select {
	case err := <-appendDone:
		if err != nil {
			t.Fatalf("AppendEdit: %v", err)
		}
	case <-time.After(sleepDur / 2):
		t.Fatal("AppendEdit did not complete before the sleeping disk hook — Materialize is holding a DB tx open across a vfs.FS call (violates the no-tx-across-disk-I/O contract)")
	}

	if err := <-matDone; err != nil {
		t.Fatalf("Materialize: %v", err)
	}
}
