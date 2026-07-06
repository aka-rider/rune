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
	// WatchSub 0 = dir-changed (triggers dir reload), 1 = file-read-error,
	// 2 = in-place external write (BUG1) — see PathIndex.
	KindWatch Kind = 5
	// KindExternalWrite simulates another process writing a file managed by
	// the workspace. The driver calls mem.WriteFile directly (no model
	// message) to advance the Mem modification clock, making the workspace's
	// baseline stale so the next ⌘S must detect divergence (§1.4.7).
	KindExternalWrite Kind = 6
	// KindExternalRename simulates another process renaming a file managed
	// by the workspace (mem.Rename(src,dst), no model message — clusters
	// script the KindWatch follow-up that surfaces it to production).
	KindExternalRename Kind = 7
	// KindExternalRemove simulates another process deleting a file managed
	// by the workspace (mem.Remove(path), no model message).
	KindExternalRemove Kind = 8
	// KindDictation injects a dictation-engine message (partial/final
	// transcription, non-fatal/fatal error) directly, bypassing the real
	// microphone/whisper pipeline (stubbed under the fuzzing build tag).
	KindDictation Kind = 9
	// KindClipboard injects a tea.ClipboardMsg — the terminal's OSC-52
	// clipboard-read response, i.e. phase 2 of super+v paste.
	KindClipboard Kind = 10
	// KindQuitRequest injects footer.ConfirmQuitMsg{} directly: the ^C^C
	// chord's completion message is structurally unreachable inline
	// (confirmDelay=0 plus synchronous drain expires the chord between the
	// two presses), so this is the only way to exercise the quit-guard path.
	KindQuitRequest Kind = 11
)

// Event is a single fuzzer-generated input to the TUI.
type Event struct {
	Kind      Kind
	KeyIndex  uint16 // KindKey: index into the binding table
	Text      string // KindText, KindPaste, KindExternalWrite, KindDictation, KindClipboard: sanitized UTF-8
	Width     uint8  // KindResize: terminal width [20,220]
	Height    uint8  // KindResize: terminal height [5,80]
	PaneIndex uint8  // KindFocus: pane index [0,4]
	PathIndex uint8  // KindWatch, KindExternalWrite, KindExternalRename, KindExternalRemove: index into the paths slice
	WatchSub  uint8  // KindWatch: 0=dir-changed 1=read-error 2=file-changed
	DestIndex uint8  // KindExternalRename: index into the paths slice for the rename destination
	DictSub   uint8  // KindDictation: sub%4 selects partial/final/non-fatal-error/fatal-error
}
