package event

// Kind identifies the type of a fuzz event.
type Kind byte

const (
	KindKey    Kind = 0
	KindText   Kind = 1
	KindPaste  Kind = 2
	KindResize Kind = 3
	KindFocus  Kind = 4
)

// Event is a single fuzzer-generated input to the TUI.
type Event struct {
	Kind      Kind
	KeyIndex  uint16 // KindKey: index into the ~90-binding table
	Text      string // KindText, KindPaste: sanitized UTF-8
	Width     uint8  // KindResize: terminal width [20,220]
	Height    uint8  // KindResize: terminal height [5,80]
	PaneIndex uint8  // KindFocus: pane index [0,4]
}
