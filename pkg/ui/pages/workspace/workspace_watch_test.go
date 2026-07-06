//go:build !fuzzing

package workspace

import (
	"testing"

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
