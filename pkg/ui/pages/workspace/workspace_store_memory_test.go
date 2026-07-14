package workspace

import (
	"testing"

	"rune/pkg/vfs"
)

// TestOpenStoreMemoryCmd_ReturnsUsableNonDegradedStore exercises
// openStoreMemoryCmd directly (bypassing the openStoreMemory func-var, which
// this package's TestMain stubs out via DisableStoreOpenForTesting for every
// other test) to prove the "None" chooser option's underlying Cmd yields a
// real, usable store that never reads as the degraded fallback.
func TestOpenStoreMemoryCmd_ReturnsUsableNonDegradedStore(t *testing.T) {
	cmd := openStoreMemoryCmd(vfs.NewMem())
	msg := cmd()

	ready, ok := msg.(StoreReadyMsg)
	if !ok {
		t.Fatalf("expected StoreReadyMsg, got %T: %+v", msg, msg)
	}
	if ready.Store == nil {
		t.Fatal("expected a non-nil Store")
	}
	if ready.Store.Degraded() {
		t.Fatal("expected Degraded()==false for a deliberate in-memory store")
	}
	if ready.Warning != "" {
		t.Fatalf("expected no Warning for a deliberate in-memory store, got %q", ready.Warning)
	}
}

// TestWorkspace_WithMemoryStore_SetsField proves WithMemoryStore is a plain
// immutable-builder flag (mirroring WithFS/WithWatcher) and defaults false —
// the property Init's branch (openStore vs openStoreMemory) actually relies
// on.
func TestWorkspace_WithMemoryStore_SetsField(t *testing.T) {
	m := newTestWorkspace(t)
	if m.memoryStore {
		t.Fatal("expected memoryStore to default false")
	}
	m = m.WithMemoryStore()
	if !m.memoryStore {
		t.Fatal("expected WithMemoryStore to set memoryStore=true")
	}
}
