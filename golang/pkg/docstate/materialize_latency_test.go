package docstate

import (
	"sync"
	"testing"
	"time"

	"rune/pkg/editor/buffer"
	"rune/pkg/vfs"
)

// TestMaterialize_NoTxAcrossDiskIO is the plan's latency guard: with a
// blocking FS hook inside Materialize's disk I/O, a concurrent-goroutine
// AppendEdit must complete WHILE the hook is still blocked. Store.perm is
// SetMaxOpenConns(1) — a single physical connection — so if Materialize held
// a DB tx open across that disk call, AppendEdit's own tx.Begin() on the
// SAME connection would necessarily block until Materialize's tx ended,
// proving the contract violated. An AppendEdit that completes while the hook
// is provably still parked is only possible if no tx spans the disk I/O.
//
// Channel handshake, not a sleep: the hook blocks on <-release until the
// test has SEEN AppendEdit complete, so there is no duration to tune and no
// schedule on which the test can flake — only a 5s watchdog for the genuine
// regression (AppendEdit wedged behind Materialize's tx).
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

	hookStarted := make(chan struct{})
	release := make(chan struct{})
	var releaseOnce sync.Once
	unblock := func() { releaseOnce.Do(func() { close(release) }) }
	defer unblock() // never leave the Materialize goroutine parked on t.Fatal

	hook := &hookFS{FS: mem, beforeExchange: func(real vfs.FS) {
		close(hookStarted)
		<-release
	}}
	s.UseFS(hook)

	matDone := make(chan error, 1)
	go func() {
		_, err := s.Materialize(loaded.DocID, path, "new content", expect.ID, 0, false)
		matDone <- err
	}()

	<-hookStarted // Materialize is now parked inside the disk call.

	appendDone := make(chan error, 1)
	go func() {
		_, err := s.AppendEdit(loaded.DocID, []buffer.AppliedEdit{{Insert: "x"}}, nil, nil)
		appendDone <- err
	}()

	select {
	case err := <-appendDone:
		// AppendEdit finished while Materialize is DEFINITELY still inside
		// the disk call (release hasn't been closed): no tx spans the I/O.
		if err != nil {
			t.Fatalf("AppendEdit: %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("AppendEdit did not complete while the disk hook was parked — Materialize is holding a DB tx open across a vfs.FS call (violates the no-tx-across-disk-I/O contract)")
	}

	unblock()
	if err := <-matDone; err != nil {
		t.Fatalf("Materialize: %v", err)
	}
}
