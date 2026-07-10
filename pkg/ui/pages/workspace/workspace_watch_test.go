package workspace

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/fsnotify/fsnotify"
)

// TestIsStructuralEvent proves the debounce drain loop's structural-upgrade
// check is scoped to the path the batch is actually about (writePath) — an
// unrelated file's churn in the same watched directory must not be able to
// mask a genuine Write to writePath as "explained away." Table-driven, no
// filesystem/timers/goroutines (CLAUDE.md §8.2).
func TestIsStructuralEvent(t *testing.T) {
	const writePath = "/watched/open.md"
	const otherPath = "/watched/decoy.tmp"

	cases := []struct {
		name string
		ev   fsnotify.Event
		want bool
	}{
		{"same-path Create", fsnotify.Event{Name: writePath, Op: fsnotify.Create}, true},
		{"same-path Remove", fsnotify.Event{Name: writePath, Op: fsnotify.Remove}, true},
		{"same-path Rename", fsnotify.Event{Name: writePath, Op: fsnotify.Rename}, true},
		{"same-path Write is not structural", fsnotify.Event{Name: writePath, Op: fsnotify.Write}, false},
		{"other-path Create must not mask our Write", fsnotify.Event{Name: otherPath, Op: fsnotify.Create}, false},
		{"other-path Remove must not mask our Write", fsnotify.Event{Name: otherPath, Op: fsnotify.Remove}, false},
		{"other-path Rename must not mask our Write", fsnotify.Event{Name: otherPath, Op: fsnotify.Rename}, false},
		{"other-path Write is irrelevant either way", fsnotify.Event{Name: otherPath, Op: fsnotify.Write}, false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := isStructuralEvent(c.ev, writePath); got != c.want {
				t.Fatalf("isStructuralEvent(%+v, %q) = %v, want %v", c.ev, writePath, got, c.want)
			}
		})
	}
}

// TestFSNotifyWatcher_WatchDir_DetectsDirChange is the only test in this
// package allowed to exercise the real fsnotify-backed Watcher end to end —
// every other test injects NoopWatcher (see newTestWorkspace) so the real
// watcher's live OS channel and goroutine never appear in an ordinary test
// run. t.Cleanup(cancel) guarantees the watcher goroutine unblocks via
// ctx.Done() and returns even if the expected event never arrives, so this
// test cannot itself leak on a slow or flaky CI runner.
func TestFSNotifyWatcher_WatchDir_DetectsDirChange(t *testing.T) {
	dir := t.TempDir()

	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)

	cmd := (FSNotifyWatcher{}).WatchDir(ctx, dir)
	msgCh := make(chan tea.Msg, 1)
	go func() { msgCh <- cmd() }()

	// Give fsnotify.Add(dir) a moment to arm before the write below — there's
	// no synchronous "watch is armed" signal to wait on instead.
	time.Sleep(50 * time.Millisecond)
	newPath := filepath.Join(dir, "new.md")
	if err := os.WriteFile(newPath, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	select {
	case msg := <-msgCh:
		if _, ok := msg.(dirChangedMsg); !ok {
			t.Fatalf("WatchDir: got %#v, want dirChangedMsg", msg)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("timed out waiting for dirChangedMsg from the real fsnotify watcher")
	}
}
