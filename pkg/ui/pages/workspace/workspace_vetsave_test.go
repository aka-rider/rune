package workspace

import "testing"

// TestVetSaveNilStoreDoesNotPanic is D2's regression test: vetSave was
// missing the nil-store guard its sibling savedObsFor has, so calling it on
// a workspace with no store attached (m.store == nil) would panic inside
// m.store.Sync — a panic that would take the unsaved buffer with it (§1.3).
// vetSave must instead refuse gracefully via SyncErr.
func TestVetSaveNilStoreDoesNotPanic(t *testing.T) {
	m := newTestWorkspace(t) // m.store is nil until StoreReadyMsg arrives
	if m.store != nil {
		t.Fatal("test setup: expected a nil store")
	}

	v := m.vetSave(1) // must not panic

	if v.SyncErr == nil {
		t.Fatal("expected vetSave to refuse with a SyncErr when m.store is nil")
	}
}
