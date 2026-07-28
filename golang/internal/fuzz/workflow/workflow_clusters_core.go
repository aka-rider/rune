package workflow

import "rune/internal/fuzz/event"

// ---- cluster implementations (0-10, the original clusters) ----

// OpenSearchAndFind: ^F → type query → Enter×k (next) → Shift+Enter×j (prev) → Esc.
func openSearchAndFind(data []byte) ([]event.Event, int) {
	if len(data) < 3 {
		return []event.Event{evSearch, evEsc}, 0
	}
	queryLen := int(data[0]%16) + 1
	nexts := int(data[1]%4) + 1
	prevs := int(data[2] % 4)
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
	redos := int(data[2] % 4)
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
		watchSub = data[1] % 3
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

// NavigateTreeAndTrash: FocusExplorer → Down×n → ⌘⌫ (TrashFile).
// When the selected entry is the dirty active document, the §1.4.4 guard
// fires instead (TRASH-DIRTY-BLOCK); otherwise the entry is optimistically
// removed and the async trash Cmd runs (TRASH-OPT-REMOVE / TRASH-TAB-GONE).
func navigateTreeAndTrash(data []byte) ([]event.Event, int) {
	downs := 2
	response := uint8(1) // default: cancel (safe)
	consumed := 0
	if len(data) >= 1 {
		downs = int(data[0]%8) + 1
		consumed = 1
	}
	if len(data) >= 2 {
		response = data[1] % 2
		consumed = 2
	}
	guardKey := event.Event{Kind: event.KindKey, KeyIndex: trashGuardResponseIndex(response)}
	var evs []event.Event
	evs = append(evs, evTree)
	evs = append(evs, repeat(evDown, downs)...)
	evs = append(evs, evTrash)
	evs = append(evs, guardKey)
	return evs, consumed
}

// NavigateTreeAndNewFile: FocusExplorer → Down×n → ^n (CreateNewFile) → FocusEditor.
// Exercises the global CreateNewFile shortcut while the filetree has focus, then
// returns focus to the editor so subsequent clusters can type into the new doc.
func navigateTreeAndNewFile(data []byte) ([]event.Event, int) {
	downs := 1
	consumed := 0
	if len(data) >= 1 {
		downs = int(data[0]%8) + 1
		consumed = 1
	}
	var evs []event.Event
	evs = append(evs, evTree)
	evs = append(evs, repeat(evDown, downs)...)
	evs = append(evs, evNew)
	evs = append(evs, evEdit)
	return evs, consumed
}
