//go:build fuzzing

// Package workflow provides DecodeWorkflow, a byte-driven cluster grammar
// that maps fuzz corpus bytes to coherent sequences of user-level events.
// Unlike event.Decode (which maps every byte to a single keystroke),
// DecodeWorkflow selects named workflow *clusters* — multi-step sequences
// that mirror real user actions (open search, navigate tree, edit+undo, etc.)
// — and parameterises them from the remaining corpus bytes.
//
// FuzzHumanSession uses this instead of event.Decode so the fuzzer exercises
// coherent flows while retaining Go-native corpus shrinking.
package workflow

import (
	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/event"
)

// DecodeWorkflow decodes a binary fuzzer input to a slice of Events using
// the cluster grammar. Every byte string produces some (possibly empty) list.
func DecodeWorkflow(data []byte) []event.Event {
	var out []event.Event
	i := 0
	for i < len(data) {
		clusterID := data[i]
		i++
		var consumed int
		var evs []event.Event
		evs, consumed = decodeCluster(clusterID, data[i:])
		i += consumed
		out = append(out, evs...)
	}
	return out
}

// decodeCluster selects one cluster by clusterID % numClusters and parameterises
// it from the prefix of data, returning the events and bytes consumed.
func decodeCluster(id byte, data []byte) ([]event.Event, int) {
	const numClusters = 9
	switch id % numClusters {
	case 0:
		return openSearchAndFind(data)
	case 1:
		return navigateTreeAndOpen(data)
	case 2:
		return editUndoRedoSave(data)
	case 3:
		return tabChurn(data)
	case 4:
		return dirtyCloseGuard(data)
	case 5:
		return externalChange(data)
	case 6:
		return resizeTerminal(data)
	case 7:
		return globalSeqDirtySpec(data)
	case 8:
		return followLink(data)
	}
	return nil, 0
}

// ---- helpers ----

func key(kp tea.KeyPressMsg) event.Event {
	// Find the index in bindingTable. We encode KindKey + 2-byte index.
	// For cluster grammar, we use hard-coded indices matching keytable.go.
	// The indices must match bindingTable in driver/keytable.go.
	return event.Event{Kind: event.KindKey, KeyIndex: keyPressToIndex(kp)}
}

// keyPressToIndex maps a well-known KeyPressMsg to its bindingTable index.
// Indices must match keytable.go exactly. Unrecognised keys map to 0 (Up).
func keyPressToIndex(kp tea.KeyPressMsg) uint16 {
	switch {
	case kp.Code == tea.KeyUp && kp.Mod == 0:
		return 0
	case kp.Code == tea.KeyDown && kp.Mod == 0:
		return 1
	case kp.Code == tea.KeyLeft && kp.Mod == 0:
		return 2
	case kp.Code == tea.KeyRight && kp.Mod == 0:
		return 3
	case kp.Code == tea.KeyHome && kp.Mod == 0:
		return 4
	case kp.Code == tea.KeyEnter && kp.Mod == 0:
		return 8
	case kp.Code == tea.KeyBackspace && kp.Mod == 0:
		return 9
	case kp.Code == tea.KeyEscape && kp.Mod == 0:
		return 32
	case kp.Code == 'w' && kp.Mod == tea.ModCtrl: // CloseFile
		return 14
	case kp.Code == 'n' && kp.Mod == tea.ModCtrl: // CreateNewFile
		return 18
	case kp.Code == 'p' && kp.Mod == tea.ModCtrl: // PinTab
		return 19
	case kp.Code == 'x' && kp.Mod == tea.ModCtrl: // FocusExplorer
		return 15
	case kp.Code == 'e' && kp.Mod == tea.ModCtrl: // FocusEditor
		return 16
	case kp.Code == 's' && kp.Mod == tea.ModSuper: // SaveFile
		return 76
	case kp.Code == 'z' && kp.Mod == tea.ModSuper: // Undo
		return 77
	case kp.Code == 'y' && kp.Mod == tea.ModCtrl: // Redo
		return 78
	// Search keys (new — appended at end of bindingTable)
	case kp.Code == 'f' && kp.Mod == tea.ModCtrl: // InFileSearch
		return 79
	case kp.Code == 'g' && kp.Mod == tea.ModSuper: // FindNext
		return 81
	case kp.Code == 'g' && kp.Mod == (tea.ModShift|tea.ModSuper): // FindPrev
		return 82
	}
	return 0
}

func textEvent(s string) event.Event {
	if len(s) > 40 {
		s = s[:40]
	}
	return event.Event{Kind: event.KindText, Text: s}
}

func repeat(ev event.Event, n int) []event.Event {
	out := make([]event.Event, n)
	for i := range out {
		out[i] = ev
	}
	return out
}

var (
	evDown     = key(tea.KeyPressMsg{Code: tea.KeyDown})
	evHome     = key(tea.KeyPressMsg{Code: tea.KeyHome})
	evEnter    = key(tea.KeyPressMsg{Code: tea.KeyEnter})
	evEsc      = key(tea.KeyPressMsg{Code: tea.KeyEscape})
	evSave     = key(tea.KeyPressMsg{Code: 's', Mod: tea.ModSuper})
	evUndo     = key(tea.KeyPressMsg{Code: 'z', Mod: tea.ModSuper})
	evRedo     = key(tea.KeyPressMsg{Code: 'y', Mod: tea.ModCtrl})
	evClose    = key(tea.KeyPressMsg{Code: 'w', Mod: tea.ModCtrl})
	evNew      = key(tea.KeyPressMsg{Code: 'n', Mod: tea.ModCtrl})
	evPin      = key(tea.KeyPressMsg{Code: 'p', Mod: tea.ModCtrl})
	evTree     = key(tea.KeyPressMsg{Code: 'x', Mod: tea.ModCtrl})
	evEdit     = key(tea.KeyPressMsg{Code: 'e', Mod: tea.ModCtrl})
	evSearch   = key(tea.KeyPressMsg{Code: 'f', Mod: tea.ModCtrl})
	evFindNext = key(tea.KeyPressMsg{Code: 'g', Mod: tea.ModSuper})
	evFindPrev = key(tea.KeyPressMsg{Code: 'g', Mod: tea.ModShift | tea.ModSuper})
)

// ---- cluster implementations ----

// OpenSearchAndFind: ^F → type query → Enter×k (next) → Shift+Enter×j (prev) → Esc.
func openSearchAndFind(data []byte) ([]event.Event, int) {
	if len(data) < 3 {
		return []event.Event{evSearch, evEsc}, 0
	}
	queryLen := int(data[0]%16) + 1
	nexts := int(data[1]%4) + 1
	prevs := int(data[2]%4)
	consumed := 3
	query := "search"
	if consumed+queryLen <= len(data) {
		query = sanitizeQuery(data[consumed : consumed+queryLen])
		consumed += queryLen
	}

	var evs []event.Event
	evs = append(evs, evSearch)
	evs = append(evs, textEvent(query))
	evs = append(evs, repeat(evFindNext, nexts)...)
	if prevs > 0 {
		evs = append(evs, repeat(evFindPrev, prevs)...)
	}
	evs = append(evs, evEsc)
	return evs, consumed
}

// NavigateTreeAndOpen: FocusExplorer → Down×n → Enter.
func navigateTreeAndOpen(data []byte) ([]event.Event, int) {
	downs := 2
	consumed := 0
	if len(data) >= 1 {
		downs = int(data[0]%8) + 1
		consumed = 1
	}
	var evs []event.Event
	evs = append(evs, evTree)
	evs = append(evs, repeat(evDown, downs)...)
	evs = append(evs, evEnter)
	return evs, consumed
}

// EditUndoRedoSave: FocusEditor → type text → Undo×u → Redo×r → ⌘S.
func editUndoRedoSave(data []byte) ([]event.Event, int) {
	if len(data) < 3 {
		return []event.Event{evEdit, evSave}, 0
	}
	textLen := int(data[0]%20) + 1
	undos := int(data[1]%4) + 1
	redos := int(data[2]%4)
	consumed := 3
	text := "hello world"
	if consumed+textLen <= len(data) {
		text = sanitizeQuery(data[consumed : consumed+textLen])
		consumed += textLen
	}

	var evs []event.Event
	evs = append(evs, evEdit)
	evs = append(evs, textEvent(text))
	evs = append(evs, repeat(evUndo, undos)...)
	if redos > 0 {
		evs = append(evs, repeat(evRedo, redos)...)
	}
	evs = append(evs, evSave)
	return evs, consumed
}

// TabChurn: ^n (new) → Close×c → Pin.
func tabChurn(data []byte) ([]event.Event, int) {
	news := 2
	closes := 1
	consumed := 0
	if len(data) >= 2 {
		news = int(data[0]%4) + 1
		closes = int(data[1]%3) + 1
		consumed = 2
	}
	var evs []event.Event
	evs = append(evs, repeat(evNew, news)...)
	evs = append(evs, evPin)
	evs = append(evs, repeat(evClose, closes)...)
	return evs, consumed
}

// DirtyCloseGuard: FocusEditor → type → ^w → one of s/d/Esc.
func dirtyCloseGuard(data []byte) ([]event.Event, int) {
	response := uint8(0)
	consumed := 0
	if len(data) >= 1 {
		response = data[0] % 3
		consumed = 1
	}
	// guard responses: 0=s (save), 1=d (discard), 2=Esc (cancel)
	guardKey := event.Event{Kind: event.KindKey, KeyIndex: guardResponseIndex(response)}
	var evs []event.Event
	evs = append(evs, evEdit)
	evs = append(evs, textEvent("dirty"))
	evs = append(evs, evClose)
	evs = append(evs, guardKey)
	return evs, consumed
}

// guardResponseIndex maps a 0–2 response to the bindingTable index for s/d/Esc.
// bindingTable: a=33, b=34, ..., d=36, ..., s=51, ..., Escape=32.
func guardResponseIndex(r uint8) uint16 {
	switch r {
	case 0:
		return 51 // 's' = save
	case 1:
		return 36 // 'd' = discard
	default:
		return 32 // Escape = cancel
	}
}

// FollowLink: FocusEditor → Down×k → Home → Enter (follow) → guard response.
// Descends to a link line in the open document and presses Enter, which follows
// the link under a lone caret instead of inserting a newline (markdownedit). Each
// seeded link starts its own line, so Home places the caret inside the link span.
// Following opens the target as a new tab; if that evicts a DIRTY background tab the
// data-loss guard appears, so a guard response (s/d/Esc) follows to drain it and keep
// the session moving. When the caret is NOT on a link, Enter inserts a newline (also
// valid) and the response key is a harmless keystroke.
func followLink(data []byte) ([]event.Event, int) {
	downs := 2
	response := uint8(2) // default Esc (cancel): keep any dirty work intact
	consumed := 0
	if len(data) >= 2 {
		downs = int(data[0]%6) + 1
		response = data[1] % 3
		consumed = 2
	}
	guardKey := event.Event{Kind: event.KindKey, KeyIndex: guardResponseIndex(response)}
	var evs []event.Event
	evs = append(evs, evEdit)
	evs = append(evs, repeat(evDown, downs)...)
	evs = append(evs, evHome)  // col 0 → inside the link span
	evs = append(evs, evEnter) // follow under a lone caret
	evs = append(evs, guardKey)
	return evs, consumed
}

// ExternalChange (fsnotify simulation cluster):
// Open file (navigate to first entry) → KindExternalWrite → KindWatch(dir-changed) →
// FocusEditor → type → ⌘S (should surface conflict).
func externalChange(data []byte) ([]event.Event, int) {
	pathIndex := uint8(0)
	watchSub := uint8(0)
	consumed := 0
	if len(data) >= 2 {
		pathIndex = data[0]
		watchSub = data[1] % 2
		consumed = 2
	}
	return []event.Event{
		// Simulate an external process writing the file.
		{Kind: event.KindExternalWrite, PathIndex: pathIndex, Text: "external-content\n"},
		// Simulate fsnotify firing.
		{Kind: event.KindWatch, PathIndex: pathIndex, WatchSub: watchSub},
		// Try to save — should be refused (FileSaveErrorMsg{Conflict:true}).
		evEdit,
		textEvent("new-content"),
		evSave,
	}, consumed
}

// ResizeTerminal: emit a small and then a large resize.
func resizeTerminal(data []byte) ([]event.Event, int) {
	w1, h1, w2, h2 := uint8(40), uint8(15), uint8(180), uint8(50)
	consumed := 0
	if len(data) >= 4 {
		w1 = clampU8(data[0], 20, 100)
		h1 = clampU8(data[1], 5, 40)
		w2 = clampU8(data[2], 80, 220)
		h2 = clampU8(data[3], 20, 80)
		consumed = 4
	}
	return []event.Event{
		{Kind: event.KindResize, Width: w1, Height: h1},
		{Kind: event.KindResize, Width: w2, Height: h2},
	}, consumed
}

// GlobalSeqDirtySpec reproduces the global-seq dirty bug:
// open file A, edit (consuming global seq), open B, edit B twice, undo×2.
// TR-dirty-clear must hold after undo.
func globalSeqDirtySpec(_ []byte) ([]event.Event, int) {
	return []event.Event{
		// File A: edit once (seq = 1)
		evEdit,
		textEvent("file-a-content"),
		// Switch to a new file (file B)
		evNew,
		evEdit,
		textEvent("file-b-edit-1"),
		textEvent("file-b-edit-2"),
		// Undo both edits on B
		evUndo,
		evUndo,
		// Save (TR-dirty-clear: after undo of all edits, B should not be dirty)
		evSave,
	}, 0
}

// ---- helpers ----

func sanitizeQuery(b []byte) string {
	out := make([]byte, 0, len(b))
	for _, c := range b {
		// Keep printable ASCII; remap control chars to letters.
		if c >= 32 && c < 127 {
			out = append(out, c)
		} else {
			out = append(out, 'x')
		}
	}
	if len(out) == 0 {
		return "q"
	}
	return string(out)
}

func clampU8(v, lo, hi uint8) uint8 {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}

