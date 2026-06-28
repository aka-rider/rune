package event

// Kind identifies the type of a fuzz event.
type Kind byte

const (
	KindKey    Kind = 0
	KindText   Kind = 1
	KindPaste  Kind = 2
	KindResize Kind = 3
	KindFocus  Kind = 4
	// KindWatch simulates an fsnotify filesystem notification.
	// WatchSub 0 = dir-changed (triggers dir reload), 1 = file-read-error.
	KindWatch Kind = 5
	// KindExternalWrite simulates another process writing a file managed by
	// the workspace. The driver calls mem.WriteFile directly (no model
	// message) to advance the Mem modification clock, making the workspace's
	// baseline stale so the next ⌘S must detect divergence (§1.4.7).
	KindExternalWrite Kind = 6
)

// Event is a single fuzzer-generated input to the TUI.
type Event struct {
	Kind      Kind
	KeyIndex  uint16 // KindKey: index into the binding table
	Text      string // KindText, KindPaste, KindExternalWrite: sanitized UTF-8
	Width     uint8  // KindResize: terminal width [20,220]
	Height    uint8  // KindResize: terminal height [5,80]
	PaneIndex uint8  // KindFocus: pane index [0,4]
	PathIndex uint8  // KindWatch, KindExternalWrite: index into the paths slice
	WatchSub  uint8  // KindWatch: 0=dir-changed 1=read-error
}
