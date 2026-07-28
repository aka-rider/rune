package main

import (
	"os"
	"path/filepath"
	"testing"

	"rune/internal/editortest"
	ui "rune/pkg/ui"
	"rune/pkg/ui/pages/workspace"
)

// TestNewApp_MemoryStore_NoRuneDirCreated is an end-to-end check of the
// rootChooser "None" option's effect on ui.NewApp: a real temp dir + real
// vfs.Disk{} (this package, unlike pkg/ui/pages/workspace's own test suite,
// never disables the real store-open path via harness.Hermetic()), proving
// Init() yields a usable, non-degraded in-memory store AND never creates
// workDir/.rune on disk.
func TestNewApp_MemoryStore_NoRuneDirCreated(t *testing.T) {
	dir := t.TempDir()

	app, err := ui.NewApp(dir, nil, true)
	if err != nil {
		t.Fatalf("NewApp: %v", err)
	}

	msgs := editortest.ExecCmds(app.Init())
	var ready *workspace.StoreReadyMsg
	for _, msg := range msgs {
		if m, ok := msg.(workspace.StoreReadyMsg); ok {
			ready = &m
		}
	}
	if ready == nil {
		t.Fatal("expected a StoreReadyMsg among Init()'s results")
	}
	if ready.Store == nil {
		t.Fatal("expected a non-nil Store")
	}
	if ready.Store.Degraded() {
		t.Fatal("expected Degraded()==false for an intentional in-memory store")
	}

	if _, err := os.Stat(filepath.Join(dir, ".rune")); !os.IsNotExist(err) {
		t.Fatalf("expected no .rune directory to be created under %s, stat err = %v", dir, err)
	}
}
