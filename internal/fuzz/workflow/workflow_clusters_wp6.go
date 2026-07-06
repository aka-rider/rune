//go:build fuzzing

// WP6 input-surface clusters (16-19): selection/clipboard, unicode typing,
// dictation, and workspace chrome. See workflow.go for the shared helpers
// (paletteText, event vars) these clusters use.
package workflow

import "rune/internal/fuzz/event"

// SelectionClipboard [op,t]: FocusEditor → type palette text → select it
// (SelectAll / Shift+Right×3 / Alt+Shift+Right — varied by t%3) → one of 4
// actions (op%4) → Undo → Save.
//
//	0 Copy (⌘C — textedit's clipboardCopy command).
//	1 Cut (⌘X — clipboardCut) then a KindClipboard paste-over (tea.ClipboardMsg,
//	  the OSC-52 response phase — handlePasteContent).
//	2 MoveLineUp then MoveLineDown (⌥↑/⌥↓ — edit.move-line-* commands).
//	3 AddCursorBelow (⌥⌘↓) then a multi-line KindClipboard paste — exercises
//	  handlePasteContent's per-cursor distribute — then Esc
//	  (MulticursorEscape) collapses back to one cursor.
func selectionClipboard(data []byte) ([]event.Event, int) {
	op := uint8(0)
	t := uint8(0)
	consumed := 0
	if len(data) >= 2 {
		op = data[0] % 4
		t = data[1]
		consumed = 2
	}
	var evs []event.Event
	evs = append(evs, evEdit)
	evs = append(evs, textEvent(paletteText(t)))
	switch t % 3 {
	case 0:
		evs = append(evs, evSelectAll)
	case 1:
		evs = append(evs, repeat(evShiftRight, 3)...)
	default:
		evs = append(evs, evShiftWordRight)
	}
	switch op {
	case 0: // Copy
		evs = append(evs, evCopy)
	case 1: // Cut + paste-over
		evs = append(evs, evCut)
		evs = append(evs, event.Event{Kind: event.KindClipboard, Text: paletteText(t + 1)})
	case 2: // MoveLineUp/Down
		evs = append(evs, evMoveLineUp, evMoveLineDown)
	default: // AddCursorBelow + multi-line paste distribute + Esc
		evs = append(evs, evAddCursorDown)
		evs = append(evs, event.Event{Kind: event.KindClipboard, Text: "x\ny\nz"})
		evs = append(evs, evEsc)
	}
	evs = append(evs, evUndo)
	evs = append(evs, evSave)
	return evs, consumed
}

// UnicodeTyping [t1,t2,k]: FocusEditor → paste palette text t1 → k multi-byte
// character keys (CJK/emoji/precomposed accent, cycling — bindingTable
// indices 108-110, edit.insert-character's non-paste multi-byte path) →
// Left → Backspace (deletes across whatever rune boundary the caret landed
// on) → paste palette text t2 → Save → Undo.
func unicodeTyping(data []byte) ([]event.Event, int) {
	t1 := uint8(0)
	t2 := uint8(0)
	k := uint8(0)
	consumed := 0
	if len(data) >= 3 {
		t1 = data[0]
		t2 = data[1]
		k = data[2]
		consumed = 3
	}
	multiByte := []event.Event{evCJK, evEmoji, evAccented}
	var evs []event.Event
	evs = append(evs, evEdit)
	evs = append(evs, textEvent(paletteText(t1)))
	n := int(k%6) + 1
	for i := 0; i < n; i++ {
		evs = append(evs, multiByte[i%len(multiByte)])
	}
	evs = append(evs, evLeft)
	evs = append(evs, evBackspace)
	evs = append(evs, textEvent(paletteText(t2)))
	evs = append(evs, evSave)
	evs = append(evs, evUndo)
	return evs, consumed
}

// Dictation [t,v]: FocusEditor → seed text → ^V starts a session
// (footer.DictationStartMsg → m.dict.Enable + the fuzzing StartCmd stub) →
// KindDictation events, varied by v%4:
//
//	0 happy path: Partial(accumulated) → Final(same/longer, non-empty) —
//	  applies cleanly, session ends normally.
//	1 empty-reset hazard: Partial(non-empty, applied — appliedLen>0) →
//	  Final(""), the exact shape that used to destructively clear the
//	  buffer (dictation.go's FinalTranscriptionMsg case lacked the
//	  TrimSpace guard the Partial case above it already had — fixed this
//	  work package; DICT-NO-DESTROY's backstop covers it going forward).
//	2 stale-ticket: Partial(applied) → ^N (a fresh untitled, changing
//	  m.view.DocID/epoch — Part IV's ticket invalidation) → another
//	  Partial/Final arrives for the OLD session and must be ignored (the
//	  page validates dict.Ticket() before draining a pending edit).
//	3 errors: a transient ErrorMsg (non-fatal — session keeps listening)
//	  followed by a fatal ErrorMsg (ends the session, DoneMsg emitted).
func dictationCluster(data []byte) ([]event.Event, int) {
	t := uint8(0)
	v := uint8(0)
	consumed := 0
	if len(data) >= 2 {
		t = data[0]
		v = data[1] % 4
		consumed = 2
	}
	seed := paletteText(t)
	var evs []event.Event
	evs = append(evs, evEdit)
	evs = append(evs, textEvent(seed))
	evs = append(evs, evVoiceDictate) // ^V — start session

	partial := func(s string) event.Event {
		return event.Event{Kind: event.KindDictation, DictSub: 0, Text: s}
	}
	final := func(s string) event.Event {
		return event.Event{Kind: event.KindDictation, DictSub: 1, Text: s}
	}
	errTransient := event.Event{Kind: event.KindDictation, DictSub: 2}
	errFatal := event.Event{Kind: event.KindDictation, DictSub: 3}

	switch v {
	case 0: // happy path
		evs = append(evs, partial("dictated partial"))
		evs = append(evs, final("dictated final"))
	case 1: // empty-reset hazard
		evs = append(evs, partial("dictated partial"))
		evs = append(evs, final(""))
	case 2: // stale-ticket
		evs = append(evs, partial("dictated partial"))
		evs = append(evs, evNew) // fresh untitled — invalidates the session's ticket
		evs = append(evs, partial("stale partial"))
		evs = append(evs, final("stale final"))
	default: // errors
		evs = append(evs, errTransient)
		evs = append(evs, errFatal)
	}
	evs = append(evs, evSave)
	return evs, consumed
}

// WorkspaceChrome [d]: TabSwitch(d%10) → PinTab → ZenMode×2 (toggle on/off) →
// Help(F1)×2 (toggle on/off) → FocusChat + type chat text (journalEdit
// "chat") → a single ^C (chord arm — the ^C^C quit chord's FIRST press only;
// a second, completing press is exercised by the quitSaveAll cluster's own
// KindQuitRequest injection, since a real two-press chord race is what the
// fuzzer can reach reliably here) → PgDown → ^U (HalfPageUp) → Esc →
// FocusEditor.
func workspaceChrome(data []byte) ([]event.Event, int) {
	d := uint8(0)
	consumed := 0
	if len(data) >= 1 {
		d = data[0]
		consumed = 1
	}
	var evs []event.Event
	evs = append(evs, event.Event{Kind: event.KindKey, KeyIndex: tabSwitchIndex(d)})
	evs = append(evs, evPin)
	evs = append(evs, evZenMode, evZenMode)
	evs = append(evs, evHelp, evHelp)
	evs = append(evs, evFocusChat)
	evs = append(evs, textEvent("chat message"))
	evs = append(evs, evConfirmExitC)
	evs = append(evs, evPgDown)
	evs = append(evs, evHalfPageUp)
	evs = append(evs, evEsc)
	evs = append(evs, evEdit)
	return evs, consumed
}
